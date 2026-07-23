use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerEvent {
    LayerSelected(usize),
    ComboArmed(usize),
    DoubleTapped(usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyRoute {
    Base { layer: usize, key: usize },
    Combo { layer: usize, key: usize },
    Suppressed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyGestureEvent {
    Tap,
    Hold,
    DoubleTap,
}

#[derive(Clone, Copy, Debug)]
struct KeyPress {
    pressed_at: Instant,
    hold_enabled: bool,
    double_tap_enabled: bool,
    second_tap: bool,
    hold_emitted: bool,
}

#[derive(Default)]
pub struct KeyGestureController {
    press: Option<KeyPress>,
    pending_tap_at: Option<Instant>,
}

impl KeyGestureController {
    pub fn pressed(
        &mut self,
        now: Instant,
        hold_enabled: bool,
        double_tap_enabled: bool,
        double_tap_duration: Duration,
    ) -> Option<KeyGestureEvent> {
        if self.press.is_some() {
            return None;
        }
        let second_tap = double_tap_enabled
            && self
                .pending_tap_at
                .is_some_and(|released_at| now.duration_since(released_at) <= double_tap_duration);
        if second_tap {
            self.pending_tap_at = None;
        }
        self.press = Some(KeyPress {
            pressed_at: now,
            hold_enabled,
            double_tap_enabled,
            second_tap,
            hold_emitted: false,
        });
        (!hold_enabled && !double_tap_enabled).then_some(KeyGestureEvent::Tap)
    }

    pub fn released(&mut self, now: Instant) -> Option<KeyGestureEvent> {
        let press = self.press.take()?;
        if press.hold_emitted {
            return None;
        }
        if press.second_tap {
            return Some(KeyGestureEvent::DoubleTap);
        }
        if press.double_tap_enabled {
            self.pending_tap_at = Some(now);
            return None;
        }
        press.hold_enabled.then_some(KeyGestureEvent::Tap)
    }

    pub fn update(
        &mut self,
        now: Instant,
        hold_duration: Duration,
        double_tap_duration: Duration,
    ) -> Option<KeyGestureEvent> {
        if let Some(press) = self.press.as_mut()
            && press.hold_enabled
            && !press.hold_emitted
            && now.duration_since(press.pressed_at) >= hold_duration
        {
            press.hold_emitted = true;
            self.pending_tap_at = None;
            return Some(KeyGestureEvent::Hold);
        }
        if self.press.is_none()
            && self
                .pending_tap_at
                .is_some_and(|released_at| now.duration_since(released_at) >= double_tap_duration)
        {
            self.pending_tap_at = None;
            return Some(KeyGestureEvent::Tap);
        }
        None
    }

    pub const fn is_idle(&self) -> bool {
        self.press.is_none() && self.pending_tap_at.is_none()
    }
}

#[derive(Clone, Copy, Debug)]
struct HoldGesture {
    layer: usize,
    pressed_at: Instant,
    armed: bool,
    second_tap: bool,
    double_tap_enabled: bool,
}

#[derive(Clone, Copy, Debug)]
struct PendingTap {
    layer: usize,
    released_at: Instant,
}

pub struct LayerController {
    active_layer: usize,
    hold: Option<HoldGesture>,
    pending_tap: Option<PendingTap>,
    suppress_until_release: bool,
}

impl LayerController {
    pub const fn new(active_layer: usize) -> Self {
        Self {
            active_layer,
            hold: None,
            pending_tap: None,
            suppress_until_release: false,
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

    pub const fn gesture_active(&self) -> bool {
        self.hold.is_some()
    }

    pub fn suppress_keys_until_release(&mut self) {
        self.suppress_until_release = true;
    }

    pub fn button_pressed(
        &mut self,
        layer: usize,
        now: Instant,
        double_tap_duration: Duration,
        double_tap_enabled: bool,
    ) -> Option<ControllerEvent> {
        if self.hold.is_some() {
            return None;
        }
        let second_tap = double_tap_enabled
            && self.pending_tap.is_some_and(|pending| {
                pending.layer == layer
                    && now.duration_since(pending.released_at) <= double_tap_duration
            });
        let event = if second_tap {
            self.pending_tap = None;
            None
        } else {
            self.commit_pending_tap()
        };
        self.hold = Some(HoldGesture {
            layer,
            pressed_at: now,
            armed: false,
            second_tap,
            double_tap_enabled,
        });
        event
    }

    pub fn update(
        &mut self,
        now: Instant,
        hold_duration: Duration,
        double_tap_duration: Duration,
    ) -> Option<ControllerEvent> {
        if let Some(gesture) = self.hold.as_mut()
            && !gesture.armed
            && now.duration_since(gesture.pressed_at) >= hold_duration
        {
            gesture.armed = true;
            return Some(ControllerEvent::ComboArmed(gesture.layer));
        }
        if self.hold.is_none()
            && self.pending_tap.is_some_and(|pending| {
                now.duration_since(pending.released_at) >= double_tap_duration
            })
        {
            return self.commit_pending_tap();
        }
        None
    }

    pub fn button_released(&mut self, layer: usize, now: Instant) -> Option<ControllerEvent> {
        let gesture = self.hold?;
        if gesture.layer != layer {
            return None;
        }
        self.hold = None;
        self.suppress_until_release = false;
        if gesture.armed {
            return None;
        }
        if gesture.second_tap {
            return Some(ControllerEvent::DoubleTapped(layer));
        }
        if !gesture.double_tap_enabled {
            self.active_layer = layer;
            return Some(ControllerEvent::LayerSelected(layer));
        }
        self.pending_tap = Some(PendingTap {
            layer,
            released_at: now,
        });
        None
    }

    pub fn key_pressed(&mut self, key: usize) -> KeyRoute {
        if self.suppress_until_release {
            return KeyRoute::Suppressed;
        }
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

    fn commit_pending_tap(&mut self) -> Option<ControllerEvent> {
        let pending = self.pending_tap.take()?;
        self.active_layer = pending.layer;
        Some(ControllerEvent::LayerSelected(pending.layer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOLD: Duration = Duration::from_millis(350);
    const DOUBLE_TAP: Duration = Duration::from_millis(250);

    #[test]
    fn tap_selects_a_persistent_layer() {
        let now = Instant::now();
        let mut controller = LayerController::new(0);
        assert_eq!(controller.button_pressed(1, now, DOUBLE_TAP, true), None);
        assert_eq!(
            controller.button_released(1, now + Duration::from_millis(40)),
            None
        );
        assert_eq!(
            controller.update(
                now + DOUBLE_TAP + Duration::from_millis(40),
                HOLD,
                DOUBLE_TAP
            ),
            Some(ControllerEvent::LayerSelected(1))
        );
        assert_eq!(controller.active_layer(), 1);
    }

    #[test]
    fn held_combo_routes_keys_without_changing_the_persistent_layer() {
        let now = Instant::now();
        let mut controller = LayerController::new(2);
        controller.button_pressed(0, now, DOUBLE_TAP, false);
        assert_eq!(controller.key_pressed(1), KeyRoute::Suppressed);
        assert_eq!(
            controller.update(now + HOLD, HOLD, DOUBLE_TAP),
            Some(ControllerEvent::ComboArmed(0))
        );
        assert_eq!(
            controller.key_pressed(1),
            KeyRoute::Combo { layer: 0, key: 1 }
        );
        assert_eq!(controller.button_released(0, now + HOLD), None);
        assert_eq!(controller.active_layer(), 2);
    }

    #[test]
    fn an_armed_hold_without_a_key_preserves_the_persistent_layer() {
        let now = Instant::now();
        let mut controller = LayerController::new(1);
        controller.button_pressed(0, now, DOUBLE_TAP, false);
        assert_eq!(
            controller.update(now + HOLD, HOLD, DOUBLE_TAP),
            Some(ControllerEvent::ComboArmed(0))
        );
        assert_eq!(controller.button_released(0, now + HOLD), None);
        assert_eq!(controller.active_layer(), 1);
    }

    #[test]
    fn a_modal_action_suppresses_keys_until_the_layer_button_is_released() {
        let now = Instant::now();
        let mut controller = LayerController::new(0);
        controller.button_pressed(1, now, DOUBLE_TAP, false);
        controller.update(now + HOLD, HOLD, DOUBLE_TAP);
        controller.suppress_keys_until_release();
        assert_eq!(controller.key_pressed(0), KeyRoute::Suppressed);
        assert_eq!(controller.button_released(1, now + HOLD), None);
        assert_eq!(
            controller.key_pressed(0),
            KeyRoute::Base { layer: 0, key: 0 }
        );
    }

    #[test]
    fn two_quick_taps_emit_a_double_tap_without_selecting_the_layer() {
        let now = Instant::now();
        let mut controller = LayerController::new(2);
        controller.button_pressed(1, now, DOUBLE_TAP, true);
        assert_eq!(
            controller.button_released(1, now + Duration::from_millis(30)),
            None
        );
        assert_eq!(
            controller.button_pressed(1, now + Duration::from_millis(100), DOUBLE_TAP, true),
            None
        );
        assert_eq!(
            controller.button_released(1, now + Duration::from_millis(130)),
            Some(ControllerEvent::DoubleTapped(1))
        );
        assert_eq!(controller.active_layer(), 2);
    }

    #[test]
    fn pressing_another_button_commits_the_pending_single_tap() {
        let now = Instant::now();
        let mut controller = LayerController::new(0);
        controller.button_pressed(1, now, DOUBLE_TAP, true);
        controller.button_released(1, now + Duration::from_millis(30));
        assert_eq!(
            controller.button_pressed(2, now + Duration::from_millis(100), DOUBLE_TAP, false),
            Some(ControllerEvent::LayerSelected(1))
        );
        assert_eq!(controller.active_layer(), 1);
    }

    #[test]
    fn a_button_without_a_double_tap_binding_selects_immediately() {
        let now = Instant::now();
        let mut controller = LayerController::new(0);
        controller.button_pressed(2, now, DOUBLE_TAP, false);
        assert_eq!(
            controller.button_released(2, now + Duration::from_millis(30)),
            Some(ControllerEvent::LayerSelected(2))
        );
    }

    #[test]
    fn a_plain_he_key_fires_on_press() {
        let now = Instant::now();
        let mut key = KeyGestureController::default();
        assert_eq!(
            key.pressed(now, false, false, DOUBLE_TAP),
            Some(KeyGestureEvent::Tap)
        );
        assert_eq!(key.released(now + Duration::from_millis(20)), None);
        assert!(key.is_idle());
    }

    #[test]
    fn an_he_key_hold_suppresses_its_tap() {
        let now = Instant::now();
        let mut key = KeyGestureController::default();
        assert_eq!(key.pressed(now, true, false, DOUBLE_TAP), None);
        assert_eq!(
            key.update(now + HOLD, HOLD, DOUBLE_TAP),
            Some(KeyGestureEvent::Hold)
        );
        assert_eq!(key.released(now + HOLD), None);
    }

    #[test]
    fn an_he_key_double_tap_suppresses_its_single_tap() {
        let now = Instant::now();
        let mut key = KeyGestureController::default();
        assert_eq!(key.pressed(now, false, true, DOUBLE_TAP), None);
        assert_eq!(key.released(now + Duration::from_millis(20)), None);
        assert_eq!(
            key.pressed(now + Duration::from_millis(80), false, true, DOUBLE_TAP),
            None
        );
        assert_eq!(
            key.released(now + Duration::from_millis(100)),
            Some(KeyGestureEvent::DoubleTap)
        );
        assert!(key.is_idle());
    }

    #[test]
    fn an_he_key_single_tap_fires_after_the_double_tap_window() {
        let now = Instant::now();
        let mut key = KeyGestureController::default();
        key.pressed(now, false, true, DOUBLE_TAP);
        key.released(now + Duration::from_millis(20));
        assert_eq!(
            key.update(
                now + Duration::from_millis(20) + DOUBLE_TAP,
                HOLD,
                DOUBLE_TAP
            ),
            Some(KeyGestureEvent::Tap)
        );
    }
}
