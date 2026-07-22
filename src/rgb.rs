use std::{collections::HashMap, thread, time::Duration};

use anyhow::{Context, Result, anyhow};
use hidapi::{HidApi, HidDevice};

use crate::hardware::{Position, WOOTING_VID};

const UWU_PRODUCT_FAMILY: u16 = 0x1510;
const PRODUCT_MODE_MASK: u16 = 0xfff0;
const RGB_USAGE_PAGE_V3: u16 = 0xff55;
const COLOR_INIT_COMMAND: u8 = 33;
const RESET_ALL_COMMAND: u8 = 32;
const RAW_COLORS_REPORT: u8 = 11;
const V3_REPORT_SIZE: usize = 2047;
const V3_RESPONSE_SIZE: usize = 2046;
const FEATURE_RESPONSE_TIMEOUT_MS: i32 = 1000;

pub const LED_POSITIONS: [Position; 20] = [
    Position::new(0, 0),
    Position::new(0, 2),
    Position::new(0, 4),
    Position::new(0, 6),
    Position::new(1, 0),
    Position::new(1, 6),
    Position::new(2, 0),
    Position::new(2, 1),
    Position::new(2, 3),
    Position::new(2, 5),
    Position::new(2, 6),
    Position::new(3, 0),
    Position::new(3, 2),
    Position::new(3, 3),
    Position::new(3, 4),
    Position::new(3, 6),
    Position::new(4, 1),
    Position::new(4, 2),
    Position::new(4, 4),
    Position::new(4, 5),
];

pub const EDGE_POSITIONS: [Position; 14] = [
    Position::new(0, 0),
    Position::new(0, 2),
    Position::new(0, 4),
    Position::new(0, 6),
    Position::new(1, 0),
    Position::new(1, 6),
    Position::new(2, 0),
    Position::new(2, 6),
    Position::new(3, 0),
    Position::new(3, 6),
    Position::new(4, 1),
    Position::new(4, 2),
    Position::new(4, 4),
    Position::new(4, 5),
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn from_hex(value: &str) -> Result<Self> {
        let value = value.strip_prefix('#').unwrap_or(value);
        if value.len() != 6 {
            return Err(anyhow!("expected six hex digits"));
        }
        Ok(Self(
            u8::from_str_radix(&value[0..2], 16)?,
            u8::from_str_radix(&value[2..4], 16)?,
            u8::from_str_radix(&value[4..6], 16)?,
        ))
    }

    pub fn scale(self, amount: f32) -> Self {
        let amount = amount.clamp(0.0, 1.0);
        Self(
            (f32::from(self.0) * amount) as u8,
            (f32::from(self.1) * amount) as u8,
            (f32::from(self.2) * amount) as u8,
        )
    }
}

pub struct UwURgb {
    device: HidDevice,
    last_frame: HashMap<Position, Rgb>,
    restored: bool,
}

impl UwURgb {
    pub fn open(api: &HidApi) -> Result<Self> {
        let info = api.device_list().find(|info| {
            info.vendor_id() == WOOTING_VID
                && (info.product_id() & PRODUCT_MODE_MASK) == UWU_PRODUCT_FAMILY
                && info.usage_page() == RGB_USAGE_PAGE_V3
        }).context("Wooting UwU RGB interface (usage page 0xFF55) not found; the non-RGB UwU is not supported")?;
        let device = info
            .open_device(api)
            .context("failed to open Wooting UwU RGB interface")?;
        let this = Self {
            device,
            last_frame: HashMap::new(),
            restored: false,
        };
        this.send_feature(COLOR_INIT_COMMAND)?;
        Ok(this)
    }

    fn send_feature(&self, command: u8) -> Result<()> {
        let report = [1, 0xd1, 0xda, command, 0, 0, 0, 0];
        self.device
            .send_feature_report(&report)
            .with_context(|| format!("failed to send Wooting feature command {command}"))?;
        let mut response = vec![0_u8; V3_RESPONSE_SIZE];
        let received = self
            .device
            .read_timeout(&mut response, FEATURE_RESPONSE_TIMEOUT_MS)
            .with_context(|| format!("failed reading response to Wooting command {command}"))?;
        anyhow::ensure!(
            received >= 4,
            "short Wooting response for command {command}: {received} bytes"
        );
        anyhow::ensure!(
            response[..4] == [1, 0xd1, 0xda, command],
            "unexpected Wooting response for command {command}: {:02x?}",
            &response[..received.min(8)]
        );
        Ok(())
    }

    pub fn set_frame(&mut self, colors: &HashMap<Position, Rgb>) -> Result<()> {
        if colors == &self.last_frame {
            return Ok(());
        }
        let mut report = vec![0_u8; V3_REPORT_SIZE];
        report[0] = 5;
        report[1] = 0xd1;
        report[2] = 0xda;
        report[3] = RAW_COLORS_REPORT;
        for (&position, &color) in colors {
            if position.row >= 6 || position.col >= 21 {
                continue;
            }
            let encoded = encode_rgb565(color).to_le_bytes();
            let offset = 4 + (usize::from(position.row) * 21 + usize::from(position.col)) * 2;
            report[offset] = encoded[0];
            report[offset + 1] = encoded[1];
        }
        let written = self
            .device
            .write(&report)
            .context("failed to write Wooting RGB frame")?;
        anyhow::ensure!(
            written == report.len(),
            "short Wooting RGB write: {written} bytes"
        );
        self.last_frame.clone_from(colors);
        Ok(())
    }

    pub fn reset(&mut self) -> Result<()> {
        if self.restored {
            return Ok(());
        }
        self.send_feature(RESET_ALL_COMMAND)?;
        thread::sleep(Duration::from_millis(30));
        self.restored = true;
        Ok(())
    }
}

impl Drop for UwURgb {
    fn drop(&mut self) {
        if !self.restored {
            let _ = self.reset();
        }
    }
}

fn encode_rgb565(color: Rgb) -> u16 {
    (u16::from(color.0 & 0xf8) << 8) | (u16::from(color.1 & 0xfc) << 3) | u16::from(color.2 >> 3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_scales_hex_color() {
        assert_eq!(Rgb::from_hex("#ff8040").unwrap(), Rgb(255, 128, 64));
        assert_eq!(Rgb(200, 100, 50).scale(0.5), Rgb(100, 50, 25));
    }

    #[test]
    fn rgb565_matches_standard_encoding() {
        assert_eq!(encode_rgb565(Rgb(255, 0, 0)), 0xf800);
        assert_eq!(encode_rgb565(Rgb(0, 255, 0)), 0x07e0);
        assert_eq!(encode_rgb565(Rgb(0, 0, 255)), 0x001f);
    }
}
