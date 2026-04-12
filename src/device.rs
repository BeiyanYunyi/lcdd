use anyhow::{Context, Result, anyhow, bail, ensure};
use hidapi::{HidApi, HidDevice};
use log::{debug, info};

use crate::config::{AppConfig, DeviceConfig};
use crate::image::PreparedImage;
use crate::protocol::{ACK_PACKET_LEN, ACK_SIGNATURE, INIT_PACKET};

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
pub struct DeviceSession {
    init: HidDevice,
    bulk: HidDevice,
    ack_timeout_ms: i32,
}

impl DeviceSession {
    pub fn open(api: &HidApi, config: &AppConfig) -> Result<Self> {
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

    pub fn initialize(&self) -> Result<()> {
        write_exact(&self.init, &INIT_PACKET).context("failed to write init packet")?;
        Ok(())
    }

    pub fn upload_image(&self, image: &PreparedImage) -> Result<()> {
        self.drain_acks()?;
        for packet in image.packets() {
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
