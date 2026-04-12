use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail, ensure};
use config::{Config, File};
use hidapi::{HidApi, HidDevice};
use log::{debug, info, warn};
use serde::Deserialize;

const DEFAULT_VENDOR_ID: u16 = 0x0b05;
const DEFAULT_PRODUCT_ID: u16 = 0x1ca9;
const DEFAULT_INIT_INTERFACE: i32 = 0;
const DEFAULT_BULK_INTERFACE: i32 = 1;
const DEFAULT_REFRESH_INTERVAL_MS: u64 = 0;
const DEFAULT_ACK_TIMEOUT_MS: i32 = 2000;
const DEFAULT_RETRY_DELAY_MS: u64 = 1000;
const DEFAULT_RELOAD_CHECK_INTERVAL_MS: u64 = 500;
const INIT_PACKET_LEN: usize = 440;
const HID_PACKET_LEN: usize = 1024;
const HID_PAYLOAD_LEN: usize = HID_PACKET_LEN - 4;
const ACK_PACKET_LEN: usize = 16;
const MAX_SYNTHETIC_CHUNKS: usize = 25;
const EXPECTED_JPEG_WIDTH: u16 = 320;
const EXPECTED_JPEG_HEIGHT: u16 = 320;
const ACK_SIGNATURE: [u8; ACK_PACKET_LEN] = [
    0x08, 0x81, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
const INIT_PACKET: [u8; INIT_PACKET_LEN] = {
    let mut packet = [0u8; INIT_PACKET_LEN];
    packet[0] = 0x12;
    packet[1] = 0x01;
    packet[2] = 0x00;
    packet[3] = 0x80;
    packet[4] = 0x64;
    packet
};

#[derive(Debug, Clone, Deserialize)]
struct AppConfig {
    #[serde(default)]
    device: DeviceConfig,
    source: SourceConfig,
    #[serde(default)]
    refresh: RefreshConfig,
    #[serde(default)]
    protocol: ProtocolConfig,
}

#[derive(Debug, Clone, Deserialize)]
struct DeviceConfig {
    #[serde(default = "default_vendor_id")]
    vendor_id: u16,
    #[serde(default = "default_product_id")]
    product_id: u16,
    #[serde(default = "default_init_interface")]
    interface_init: i32,
    #[serde(default = "default_bulk_interface")]
    interface_bulk: i32,
    #[serde(default)]
    serial: Option<String>,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            vendor_id: default_vendor_id(),
            product_id: default_product_id(),
            interface_init: default_init_interface(),
            interface_bulk: default_bulk_interface(),
            serial: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct SourceConfig {
    path: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
struct RefreshConfig {
    #[serde(default = "default_refresh_interval_ms")]
    interval_ms: u64,
    #[serde(default = "default_ack_timeout_ms")]
    ack_timeout_ms: i32,
    #[serde(default = "default_retry_delay_ms")]
    retry_delay_ms: u64,
    #[serde(default = "default_reload_check_interval_ms")]
    reload_check_interval_ms: u64,
}

impl Default for RefreshConfig {
    fn default() -> Self {
        Self {
            interval_ms: default_refresh_interval_ms(),
            ack_timeout_ms: default_ack_timeout_ms(),
            retry_delay_ms: default_retry_delay_ms(),
            reload_check_interval_ms: default_reload_check_interval_ms(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ProtocolConfig {
    #[serde(default = "default_true")]
    init_on_connect: bool,
}

impl Default for ProtocolConfig {
    fn default() -> Self {
        Self {
            init_on_connect: true,
        }
    }
}

fn default_vendor_id() -> u16 {
    DEFAULT_VENDOR_ID
}

fn default_product_id() -> u16 {
    DEFAULT_PRODUCT_ID
}

fn default_init_interface() -> i32 {
    DEFAULT_INIT_INTERFACE
}

fn default_bulk_interface() -> i32 {
    DEFAULT_BULK_INTERFACE
}

fn default_refresh_interval_ms() -> u64 {
    DEFAULT_REFRESH_INTERVAL_MS
}

fn default_ack_timeout_ms() -> i32 {
    DEFAULT_ACK_TIMEOUT_MS
}

fn default_retry_delay_ms() -> u64 {
    DEFAULT_RETRY_DELAY_MS
}

fn default_reload_check_interval_ms() -> u64 {
    DEFAULT_RELOAD_CHECK_INTERVAL_MS
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone)]
struct PreparedImage {
    source_path: PathBuf,
    jpeg_bytes: Vec<u8>,
    packets: Vec<[u8; HID_PACKET_LEN]>,
    width: u16,
    height: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JpegCompatibility {
    width: u16,
    height: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct JpegComponent {
    id: u8,
    h_sample: u8,
    v_sample: u8,
    quant_table: u8,
}

trait ImageSource {
    fn current(&self) -> &PreparedImage;
    fn refresh_if_changed(&mut self) -> Result<Option<&PreparedImage>>;
}

struct WatchedFileSource {
    path: PathBuf,
    reload_interval: Duration,
    next_check_at: Instant,
    current: PreparedImage,
}

impl WatchedFileSource {
    fn new(path: PathBuf, reload_interval: Duration) -> Result<Self> {
        let current = load_prepared_image(&path)?;
        Ok(Self {
            path,
            reload_interval,
            next_check_at: Instant::now() + reload_interval,
            current,
        })
    }
}

impl ImageSource for WatchedFileSource {
    fn current(&self) -> &PreparedImage {
        &self.current
    }

    fn refresh_if_changed(&mut self) -> Result<Option<&PreparedImage>> {
        if Instant::now() < self.next_check_at {
            return Ok(None);
        }
        self.next_check_at = Instant::now() + self.reload_interval;

        let candidate = fs::read(&self.path)
            .with_context(|| format!("failed to read image source {}", self.path.display()))?;
        if candidate == self.current.jpeg_bytes {
            return Ok(None);
        }

        match prepare_image_bytes(&self.path, candidate) {
            Ok(next) => {
                info!(
                    "reloaded image {} ({} bytes, {} packets)",
                    next.source_path.display(),
                    next.jpeg_bytes.len(),
                    next.packets.len()
                );
                self.current = next;
                Ok(Some(&self.current))
            }
            Err(error) => {
                warn!(
                    "ignoring invalid updated image {}: {error:#}",
                    self.path.display()
                );
                Ok(None)
            }
        }
    }
}

#[derive(Debug)]
struct DeviceCandidate {
    path: CStringPath,
    serial: Option<String>,
}

#[derive(Debug)]
struct DevicePair {
    init: DeviceCandidate,
    bulk: DeviceCandidate,
}

#[derive(Debug)]
struct DeviceSession {
    init: HidDevice,
    bulk: HidDevice,
    ack_timeout_ms: i32,
}

impl DeviceSession {
    fn open(api: &HidApi, config: &AppConfig) -> Result<Self> {
        let pair = select_device_pair(api, &config.device)?;
        let init = api
            .open_path(pair.init.path.as_c_string())
            .context("failed to open init HID interface")?;
        let bulk = api
            .open_path(pair.bulk.path.as_c_string())
            .context("failed to open bulk HID interface")?;

        info!(
            "opened cooler interfaces (serial={:?}, init={}, bulk={})",
            pair.bulk.serial,
            pair.init.path.display(),
            pair.bulk.path.display()
        );

        Ok(Self {
            init,
            bulk,
            ack_timeout_ms: config.refresh.ack_timeout_ms,
        })
    }

    fn initialize(&self) -> Result<()> {
        write_exact(&self.init, &INIT_PACKET).context("failed to write init packet")?;
        Ok(())
    }

    fn upload_image(&self, image: &PreparedImage) -> Result<()> {
        self.drain_acks()?;
        for packet in &image.packets {
            write_exact(&self.bulk, packet).context("failed to write image packet")?;
        }
        self.read_ack()?;
        Ok(())
    }

    fn drain_acks(&self) -> Result<()> {
        let mut buf = [0u8; 64];
        loop {
            match self.bulk.read_timeout(&mut buf, 0) {
                Ok(0) => return Ok(()),
                Ok(size) => debug!("drained stale ack/read payload of {size} bytes"),
                Err(error) => {
                    let message = error.to_string();
                    if message.contains("timed out") || message.contains("timeout") {
                        return Ok(());
                    }
                    return Err(error).context("failed while draining stale acks");
                }
            }
        }
    }

    fn read_ack(&self) -> Result<()> {
        let mut buf = [0u8; 64];
        let size = self
            .bulk
            .read_timeout(&mut buf, self.ack_timeout_ms)
            .context("failed to read ack from cooler")?;
        ensure!(
            size == ACK_PACKET_LEN,
            "unexpected ack length {size}, expected {ACK_PACKET_LEN}"
        );
        ensure!(
            buf[..ACK_PACKET_LEN] == ACK_SIGNATURE,
            "ack mismatch: expected {:02x?}, got {:02x?}",
            ACK_SIGNATURE,
            &buf[..ACK_PACKET_LEN]
        );
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct CStringPath(Vec<u8>);

impl CStringPath {
    fn as_c_string(&self) -> &std::ffi::CStr {
        std::ffi::CStr::from_bytes_with_nul(&self.0)
            .expect("stored HID path must be nul-terminated")
    }

    fn display(&self) -> String {
        self.as_c_string().to_string_lossy().into_owned()
    }
}

fn main() -> Result<()> {
    env_logger::init();

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_flag = shutdown.clone();
    ctrlc::set_handler(move || {
        shutdown_flag.store(true, Ordering::SeqCst);
    })
    .context("failed to install signal handler")?;

    let config_path = resolve_config_path(env::args_os())?;
    let config = load_config(&config_path)?;
    info!("loaded config from {}", config_path.display());

    let mut source = WatchedFileSource::new(
        config.source.path.clone(),
        Duration::from_millis(config.refresh.reload_check_interval_ms),
    )?;
    info!(
        "loaded image {} ({} bytes, {} packets, {}x{})",
        source.current().source_path.display(),
        source.current().jpeg_bytes.len(),
        source.current().packets.len(),
        source.current().width,
        source.current().height
    );

    run_service(&config, &mut source, shutdown.as_ref())
}

fn run_service(
    config: &AppConfig,
    source: &mut dyn ImageSource,
    shutdown: &AtomicBool,
) -> Result<()> {
    let retry_delay = Duration::from_millis(config.refresh.retry_delay_ms);
    let refresh_interval = Duration::from_millis(config.refresh.interval_ms);

    while !shutdown.load(Ordering::SeqCst) {
        let api = match HidApi::new().context("failed to initialize hidapi") {
            Ok(api) => api,
            Err(error) => {
                warn!("hidapi initialization failed: {error:#}");
                sleep_with_shutdown(retry_delay, shutdown);
                continue;
            }
        };

        let session = match DeviceSession::open(&api, config) {
            Ok(session) => session,
            Err(error) => {
                warn!("cooler not ready: {error:#}");
                sleep_with_shutdown(retry_delay, shutdown);
                continue;
            }
        };

        if config.protocol.init_on_connect
            && let Err(error) = session.initialize()
        {
            warn!("failed to initialize cooler session: {error:#}");
            sleep_with_shutdown(retry_delay, shutdown);
            continue;
        }

        match run_connected_loop(source, shutdown, &session, refresh_interval) {
            Ok(()) => return Ok(()),
            Err(error) => {
                warn!("device session lost, reconnecting: {error:#}");
                sleep_with_shutdown(retry_delay, shutdown);
            }
        }
    }

    Ok(())
}

fn run_connected_loop(
    source: &mut dyn ImageSource,
    shutdown: &AtomicBool,
    session: &DeviceSession,
    refresh_interval: Duration,
) -> Result<()> {
    while !shutdown.load(Ordering::SeqCst) {
        if let Some(image) = source.refresh_if_changed()? {
            info!(
                "using refreshed image {} ({} packets)",
                image.source_path.display(),
                image.packets.len()
            );
        }

        let image = source.current();
        debug!(
            "uploading {} ({} bytes, {} packets)",
            image.source_path.display(),
            image.jpeg_bytes.len(),
            image.packets.len()
        );
        session.upload_image(image)?;

        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        if refresh_interval.is_zero() {
            continue;
        }
        sleep_with_shutdown(refresh_interval, shutdown);
    }

    if shutdown.load(Ordering::SeqCst) {
        info!("shutdown requested, stopping LCD service");
    }

    Ok(())
}

fn sleep_with_shutdown(duration: Duration, shutdown: &AtomicBool) {
    let mut remaining = duration;
    let tick = Duration::from_millis(50);
    while !remaining.is_zero() && !shutdown.load(Ordering::SeqCst) {
        let slice = remaining.min(tick);
        thread::sleep(slice);
        remaining = remaining.saturating_sub(slice);
    }
}

fn resolve_config_path(args: impl IntoIterator<Item = OsString>) -> Result<PathBuf> {
    let mut iter = args.into_iter();
    let _program = iter.next();
    let mut explicit = None;

    while let Some(arg) = iter.next() {
        if arg == "--config" {
            let value = iter
                .next()
                .ok_or_else(|| anyhow!("--config requires a path argument"))?;
            explicit = Some(PathBuf::from(value));
            continue;
        }
        if let Some(value) = arg.to_str().and_then(|text| text.strip_prefix("--config=")) {
            explicit = Some(PathBuf::from(value));
            continue;
        }
        bail!("unsupported argument {:?}; only --config is accepted", arg);
    }

    if let Some(path) = explicit {
        return Ok(path);
    }

    default_config_path(env::current_dir().context("failed to determine current directory")?)
}

fn default_config_path(cwd: PathBuf) -> Result<PathBuf> {
    for candidate in ["aura-lcd.toml", "aura-lcd.ron", "aura-lcd.corn"] {
        let path = cwd.join(candidate);
        if path.is_file() {
            return Ok(path);
        }
    }
    bail!(
        "no config file found in {} (expected aura-lcd.toml, aura-lcd.ron, or aura-lcd.corn)",
        cwd.display()
    )
}

fn load_config(path: &Path) -> Result<AppConfig> {
    ensure!(
        path.is_file(),
        "config file {} does not exist",
        path.display()
    );
    let config = Config::builder()
        .add_source(File::from(path.to_path_buf()))
        .build()
        .with_context(|| format!("failed to parse config {}", path.display()))?;
    let parsed: AppConfig = config
        .try_deserialize()
        .with_context(|| format!("failed to deserialize config {}", path.display()))?;
    Ok(parsed)
}

fn load_prepared_image(path: &Path) -> Result<PreparedImage> {
    let bytes =
        fs::read(path).with_context(|| format!("failed to read image file {}", path.display()))?;
    prepare_image_bytes(path, bytes)
}

fn prepare_image_bytes(path: &Path, bytes: Vec<u8>) -> Result<PreparedImage> {
    validate_file_extension(path)?;
    let (width, height) = jpeg_dimensions(&bytes)
        .with_context(|| format!("{} is not a supported JPEG", path.display()))?;
    ensure!(
        width == EXPECTED_JPEG_WIDTH && height == EXPECTED_JPEG_HEIGHT,
        "{} must be {}x{}, got {}x{}",
        path.display(),
        EXPECTED_JPEG_WIDTH,
        EXPECTED_JPEG_HEIGHT,
        width,
        height
    );
    let packets = packetize_jpeg(&bytes)?;
    Ok(PreparedImage {
        source_path: path.to_path_buf(),
        jpeg_bytes: bytes,
        packets,
        width,
        height,
    })
}

fn validate_file_extension(path: &Path) -> Result<()> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .ok_or_else(|| anyhow!("{} must have a .jpg or .jpeg extension", path.display()))?;
    ensure!(
        matches!(ext.as_str(), "jpg" | "jpeg"),
        "{} must have a .jpg or .jpeg extension",
        path.display()
    );
    Ok(())
}

fn packetize_jpeg(bytes: &[u8]) -> Result<Vec<[u8; HID_PACKET_LEN]>> {
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

fn select_device_pair(api: &HidApi, config: &DeviceConfig) -> Result<DevicePair> {
    let mut init_matches = Vec::new();
    let mut bulk_matches = Vec::new();

    for device in api.device_list() {
        if device.vendor_id() != config.vendor_id || device.product_id() != config.product_id {
            continue;
        }
        if let Some(serial) = config.serial.as_deref()
            && device.serial_number() != Some(serial)
        {
            continue;
        }

        let candidate = DeviceCandidate {
            path: CStringPath(device.path().to_bytes_with_nul().to_vec()),
            serial: device.serial_number().map(str::to_owned),
        };

        match device.interface_number() {
            n if n == config.interface_init => init_matches.push(candidate),
            n if n == config.interface_bulk => bulk_matches.push(candidate),
            _ => {}
        }
    }

    ensure!(
        !init_matches.is_empty(),
        "no matching init interface found for VID={:#06x} PID={:#06x}",
        config.vendor_id,
        config.product_id
    );
    ensure!(
        !bulk_matches.is_empty(),
        "no matching bulk interface found for VID={:#06x} PID={:#06x}",
        config.vendor_id,
        config.product_id
    );

    if config.serial.is_none() && (init_matches.len() > 1 || bulk_matches.len() > 1) {
        bail!("multiple matching coolers found; set device.serial in the config to disambiguate");
    }

    let init = init_matches
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("missing init interface after enumeration"))?;
    let bulk = bulk_matches
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("missing bulk interface after enumeration"))?;

    Ok(DevicePair { init, bulk })
}

fn write_exact(device: &HidDevice, payload: &[u8]) -> Result<()> {
    let written = device.write(payload)?;
    ensure!(
        written == payload.len(),
        "short HID write: wrote {written} of {} bytes",
        payload.len()
    );
    Ok(())
}

fn jpeg_dimensions(bytes: &[u8]) -> Result<(u16, u16)> {
    ensure!(bytes.len() >= 4, "JPEG too small");
    ensure!(
        bytes[0] == 0xff && bytes[1] == 0xd8,
        "missing JPEG SOI marker"
    );

    let mut index = 2usize;
    while index + 3 < bytes.len() {
        while index < bytes.len() && bytes[index] != 0xff {
            index += 1;
        }
        if index + 1 >= bytes.len() {
            break;
        }
        while index < bytes.len() && bytes[index] == 0xff {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }

        let marker = bytes[index];
        index += 1;

        if marker == 0xd9 {
            break;
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        ensure!(index + 1 < bytes.len(), "truncated JPEG marker segment");
        let segment_len = u16::from_be_bytes([bytes[index], bytes[index + 1]]) as usize;
        ensure!(
            segment_len >= 2,
            "invalid JPEG segment length {segment_len}"
        );
        ensure!(
            index + segment_len <= bytes.len(),
            "truncated JPEG segment payload"
        );

        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            ensure!(segment_len >= 7, "invalid SOF segment length {segment_len}");
            let height = u16::from_be_bytes([bytes[index + 3], bytes[index + 4]]);
            let width = u16::from_be_bytes([bytes[index + 5], bytes[index + 6]]);
            return Ok((width, height));
        }

        index += segment_len;
    }

    bail!("JPEG SOF marker not found")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_search_order_prefers_toml_then_ron_then_corn() {
        let temp = std::env::temp_dir().join(format!("aura-pcap-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let ron = temp.join("aura-lcd.ron");
        let toml = temp.join("aura-lcd.toml");
        let corn = temp.join("aura-lcd.corn");
        std::fs::write(&ron, "()").unwrap();
        std::fs::write(&toml, "").unwrap();
        std::fs::write(&corn, "").unwrap();

        assert_eq!(default_config_path(temp.clone()).unwrap(), toml);
        std::fs::remove_file(&toml).unwrap();
        assert_eq!(default_config_path(temp.clone()).unwrap(), ron);
        std::fs::remove_file(&ron).unwrap();
        assert_eq!(default_config_path(temp.clone()).unwrap(), corn);
        let _ = std::fs::remove_dir_all(&temp);
    }

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

    #[test]
    fn jpeg_dimension_parser_accepts_sample_capture_image() {
        let bytes = include_bytes!("assets/test.jpg");
        let dims = jpeg_dimensions(bytes).unwrap();
        assert_eq!(dims, (EXPECTED_JPEG_WIDTH, EXPECTED_JPEG_HEIGHT));
    }

    #[test]
    fn jpeg_dimension_parser_rejects_non_jpeg() {
        assert!(jpeg_dimensions(b"not-a-jpeg").is_err());
    }
}
