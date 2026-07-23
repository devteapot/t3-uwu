use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerEvent {
    LayerSelected(usize),
    ComboArmed(usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyRoute {
    Base { layer: usize, key: usize },
    Combo { layer: usize, key: usize },
    Suppressed,
}

#[derive(Clone, Copy, Debug)]
struct HoldGesture {
    layer: usize,
    pressed_at: Instant,
    armed: bool,
}

pub struct LayerController {
    active_layer: usize,
    hold: Option<HoldGesture>,
}

impl LayerController {
    pub const fn new(active_layer: usize) -> Self {
        Self {
            active_layer,
            hold: None,
        }
    }

    pub const fn active_layer(&self) -> usize {
        self.active_layer
    }

    pub fn combo_layer(&self) -> Option<usize> {
        self.hold
            .filter(|gesture| gesture.armed)
            .map(|gesture| gesture.layer)
    }

    pub fn button_pressed(&mut self, layer: usize, now: Instant) -> Option<ControllerEvent> {
        if self.hold.is_some() {
            return None;
        }
        self.hold = Some(HoldGesture {
            layer,
            pressed_at: now,
            armed: false,
        });
        None
    }

    pub fn update(&mut self, now: Instant, hold_duration: Duration) -> Option<ControllerEvent> {
        let gesture = self.hold.as_mut()?;
        if gesture.armed || now.duration_since(gesture.pressed_at) < hold_duration {
            return None;
        }
        gesture.armed = true;
        Some(ControllerEvent::ComboArmed(gesture.layer))
    }

    pub fn button_released(&mut self, layer: usize) -> Option<ControllerEvent> {
        let gesture = self.hold?;
        if gesture.layer != layer {
            return None;
        }
        self.hold = None;
        if gesture.armed {
            return None;
        }
        self.active_layer = layer;
        Some(ControllerEvent::LayerSelected(layer))
    }

    pub fn key_pressed(&mut self, key: usize) -> KeyRoute {
        let Some(gesture) = self.hold.as_mut() else {
            return KeyRoute::Base {
                layer: self.active_layer,
                key,
            };
        };
        if !gesture.armed {
            return KeyRoute::Suppressed;
        }
        KeyRoute::Combo {
            layer: gesture.layer,
            key,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOLD: Duration = Duration::from_millis(350);

    #[test]
    fn tap_selects_a_persistent_layer() {
        let now = Instant::now();
        let mut controller = LayerController::new(0);
        assert_eq!(controller.button_pressed(1, now), None);
        assert_eq!(
            controller.button_released(1),
            Some(ControllerEvent::LayerSelected(1))
        );
        assert_eq!(controller.active_layer(), 1);
    }

    #[test]
    fn held_combo_routes_keys_without_changing_the_persistent_layer() {
        let now = Instant::now();
        let mut controller = LayerController::new(2);
        controller.button_pressed(0, now);
        assert_eq!(controller.key_pressed(1), KeyRoute::Suppressed);
        assert_eq!(
            controller.update(now + HOLD, HOLD),
            Some(ControllerEvent::ComboArmed(0))
        );
        assert_eq!(
            controller.key_pressed(1),
            KeyRoute::Combo { layer: 0, key: 1 }
        );
        assert_eq!(controller.button_released(0), None);
        assert_eq!(controller.active_layer(), 2);
    }

    #[test]
    fn an_armed_hold_without_a_key_preserves_the_persistent_layer() {
        let now = Instant::now();
        let mut controller = LayerController::new(1);
        controller.button_pressed(0, now);
        assert_eq!(
            controller.update(now + HOLD, HOLD),
            Some(ControllerEvent::ComboArmed(0))
        );
        assert_eq!(controller.button_released(0), None);
        assert_eq!(controller.active_layer(), 1);
    }
}
