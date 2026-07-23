use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use hidapi::{HidApi, HidDevice};
use serde::{Deserialize, Serialize};

pub const WOOTING_VID: u16 = 0x31e3;
const UWU_PRODUCT_FAMILY: u16 = 0x1510;
const PRODUCT_MODE_MASK: u16 = 0xfff0;
const ANALOG_USAGE_PAGE: u16 = 0xff53;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Deserialize, Serialize)]
pub struct Position {
    pub row: u8,
    pub col: u8,
}

impl Position {
    pub const fn new(row: u8, col: u8) -> Self {
        Self { row, col }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct KeySample {
    pub position: Position,
    pub value: f32,
    pub actuated: bool,
}

pub struct UwUInput {
    device: HidDevice,
    values: HashMap<Position, KeySample>,
}

impl UwUInput {
    pub fn open(api: &HidApi) -> Result<Self> {
        let info = api
            .device_list()
            .find(|info| {
                info.vendor_id() == WOOTING_VID
                    && (info.product_id() & PRODUCT_MODE_MASK) == UWU_PRODUCT_FAMILY
                    && info.usage_page() == ANALOG_USAGE_PAGE
            })
            .context("Wooting UwU analog interface (usage page 0xFF53) not found")?;

        let device = info
            .open_device(api)
            .context("failed to open Wooting UwU analog interface")?;
        device
            .set_blocking_mode(false)
            .context("failed to make UwU input non-blocking")?;
        Ok(Self {
            device,
            values: HashMap::new(),
        })
    }

    pub fn read(&mut self, timeout_ms: i32) -> Result<&HashMap<Position, KeySample>> {
        let mut report = [0_u8; 64];
        let len = self
            .device
            .read_timeout(&mut report, timeout_ms)
            .context("failed reading Wooting analog report")?;
        if len == 0 {
            return Ok(&self.values);
        }
        if len < 4 {
            bail!("short Wooting analog report: {len} bytes");
        }

        self.values.clear();
        for bytes in report[..len].chunks_exact(4) {
            let matrix = bytes[0];
            let packed = bytes[2];
            let raw_value = (u16::from(bytes[3]) << 2) | u16::from((packed >> 6) & 0x03);
            if raw_value == 0 {
                continue;
            }
            let position = Position::new((matrix >> 5) & 0x07, matrix & 0x1f);
            self.values.insert(
                position,
                KeySample {
                    position,
                    value: f32::from(raw_value) / 1023.0,
                    actuated: packed & 0x01 != 0,
                },
            );
        }
        Ok(&self.values)
    }

    pub fn product_summary(api: &HidApi) -> Vec<String> {
        api.device_list()
            .filter(|info| {
                info.vendor_id() == WOOTING_VID
                    && (info.product_id() & PRODUCT_MODE_MASK) == UWU_PRODUCT_FAMILY
            })
            .map(|info| {
                format!(
                    "pid=0x{:04x} usage_page=0x{:04x} usage=0x{:04x} interface={}",
                    info.product_id(),
                    info.usage_page(),
                    info.usage(),
                    info.interface_number()
                )
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Trigger {
    down: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TriggerTransition {
    Pressed,
    Released,
}

impl Trigger {
    pub fn transition(
        &mut self,
        value: f32,
        press: f32,
        release: f32,
    ) -> Option<TriggerTransition> {
        if !self.down && value >= press {
            self.down = true;
            return Some(TriggerTransition::Pressed);
        }
        if self.down && value <= release {
            self.down = false;
            return Some(TriggerTransition::Released);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_has_hysteresis_and_only_fires_once() {
        let mut trigger = Trigger::default();
        assert_eq!(trigger.transition(0.2, 0.4, 0.15), None);
        assert_eq!(
            trigger.transition(0.5, 0.4, 0.15),
            Some(TriggerTransition::Pressed)
        );
        assert_eq!(trigger.transition(0.8, 0.4, 0.15), None);
        assert_eq!(trigger.transition(0.2, 0.4, 0.15), None);
        assert_eq!(
            trigger.transition(0.1, 0.4, 0.15),
            Some(TriggerTransition::Released)
        );
        assert_eq!(
            trigger.transition(0.5, 0.4, 0.15),
            Some(TriggerTransition::Pressed)
        );
    }

    #[test]
    fn trigger_reports_release_transitions() {
        let mut trigger = Trigger::default();
        assert_eq!(
            trigger.transition(0.5, 0.4, 0.15),
            Some(TriggerTransition::Pressed)
        );
        assert_eq!(trigger.transition(0.2, 0.4, 0.15), None);
        assert_eq!(
            trigger.transition(0.1, 0.4, 0.15),
            Some(TriggerTransition::Released)
        );
    }
}
