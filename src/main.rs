mod actions;
mod codex;
mod config;
mod controls;
mod hardware;
mod rgb;
mod t3;
mod target;

use std::{
    collections::HashMap,
    io::Read,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use hidapi::HidApi;

use codex::CodexState;
use config::Config;
use controls::{ControllerEvent, KeyGestureController, KeyGestureEvent, KeyRoute, LayerController};
use hardware::{Position, Trigger, TriggerTransition, UwUInput};
use rgb::{EDGE_POSITIONS, LED_POSITIONS, Rgb, UwURgb};
use t3::T3State;
use target::{StateSnapshot, StateSource, TargetCommand, TargetId, ThreadSlot};

#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    #[arg(short, long)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List the UwU HID interfaces visible to this process.
    Diagnose {
        /// Stream pressed matrix positions and analog values.
        #[arg(long)]
        watch: bool,
        /// Print every analog sample instead of concise press/release events.
        #[arg(long, requires = "watch")]
        raw: bool,
    },
    /// Light each physical zone briefly, then restore onboard RGB.
    TestRgb,
    /// Release Wooting RGB SDK control and restore the onboard lighting effect.
    ResetRgb,
    /// Print the latest thread slots and their resolved state for one target.
    State {
        #[arg(value_enum, default_value = "t3")]
        target: TargetId,
    },
    /// Print T3 state (compatibility alias for `state t3`).
    T3State,
    /// Exchange a T3 pairing link for read-only API access and save it in Keychain.
    Pair {
        /// Pairing URL from T3. Omit it to enter the URL without shell-history exposure.
        pairing_url: Option<String>,
    },
    /// Remove uwu-vibe's saved T3 API credential from Keychain.
    Unpair,
    /// Send one supported action to a target (useful for permission testing).
    Action {
        /// For example: thread.jump.1, chat.new, or terminal.toggle.
        action: String,
        /// Target to control. Defaults to `default_target`.
        #[arg(long, value_enum)]
        target: Option<TargetId>,
    },
    /// Consume one Codex hook event from stdin and update live LED state.
    CodexHook,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load(cli.config.as_deref())?;
    config.validate()?;
    match cli.command {
        Some(Command::Diagnose { watch, raw }) => diagnose(&config, watch, raw),
        Some(Command::TestRgb) => test_rgb(&config),
        Some(Command::ResetRgb) => reset_rgb(),
        Some(Command::State { target }) => print_state(&config, target),
        Some(Command::T3State) => print_state(&config, TargetId::T3),
        Some(Command::Pair { pairing_url }) => pair_t3(pairing_url.as_deref()),
        Some(Command::Unpair) => unpair_t3(),
        Some(Command::Action { action, target }) => {
            run_one_action(&config, target.unwrap_or(config.default_target), &action)
        }
        Some(Command::CodexHook) => record_codex_hook(),
        None => run(config),
    }
}

fn diagnose(config: &Config, watch: bool, raw: bool) -> Result<()> {
    let api = HidApi::new().context("failed to initialize HID access")?;
    let interfaces = UwUInput::product_summary(&api);
    if interfaces.is_empty() {
        anyhow::bail!("no Wooting UwU found");
    }
    println!("Wooting UwU interfaces:");
    for interface in interfaces {
        println!("  {interface}");
    }
    if !watch {
        return Ok(());
    }

    println!("\nWatching input. Press Ctrl-C to stop.");
    let mut input = UwUInput::open(&api)?;
    let watched = [
        ("HE 1", config.hall_keys[0]),
        ("HE 2", config.hall_keys[1]),
        ("HE 3", config.hall_keys[2]),
        ("Layer 1", config.layer_buttons[0]),
        ("Layer 2", config.layer_buttons[1]),
        ("Layer 3", config.layer_buttons[2]),
    ];
    let mut down = [false; 6];
    loop {
        let samples = input.read(100)?;
        if raw && !samples.is_empty() {
            let mut samples = samples.values().copied().collect::<Vec<_>>();
            samples.sort_by_key(|sample| (sample.position.row, sample.position.col));
            println!(
                "{}",
                samples
                    .iter()
                    .map(|sample| format!(
                        "r{}c{}={:.3}{}",
                        sample.position.row,
                        sample.position.col,
                        sample.value,
                        if sample.actuated { "*" } else { "" }
                    ))
                    .collect::<Vec<_>>()
                    .join("  ")
            );
        } else if !raw {
            for (index, (label, position)) in watched.iter().enumerate() {
                let value = sample_value(samples, *position);
                let next_down = if down[index] {
                    value > config.release_threshold
                } else {
                    value >= config.actuation_threshold
                };
                if next_down != down[index] {
                    down[index] = next_down;
                    println!(
                        "{:<7} {:<4} r{}c{} value={value:.3}",
                        label,
                        if next_down { "DOWN" } else { "UP" },
                        position.row,
                        position.col
                    );
                }
            }
        }
    }
}

fn test_rgb(config: &Config) -> Result<()> {
    warn_if_wootility_is_running();
    let api = HidApi::new()?;
    let mut rgb = UwURgb::open(&api)?;
    for color in [Rgb(255, 40, 40), Rgb(40, 255, 100), Rgb(50, 100, 255)] {
        let frame = LED_POSITIONS
            .into_iter()
            .map(|position| (position, color.scale(config.brightness)))
            .collect();
        rgb.set_frame(&frame)?;
        thread::sleep(Duration::from_millis(500));
    }
    rgb.reset()
}

fn reset_rgb() -> Result<()> {
    let api = HidApi::new()?;
    let mut rgb = UwURgb::open(&api)?;
    rgb.reset()?;
    println!("Released RGB SDK control and restored onboard UwU lighting.");
    Ok(())
}

fn print_state(config: &Config, target: TargetId) -> Result<()> {
    let snapshot = match target {
        TargetId::T3 => T3State::open(config)?.slots()?,
        TargetId::Codex => CodexState::open(config)?.slots()?,
    };
    println!("Target: {}", target.label());
    println!("State source: {}", snapshot.source.label());
    if let Some(reason) = snapshot.degraded_reason {
        println!("Fallback reason: {reason}");
    }
    for (index, slot) in snapshot.slots.iter().enumerate() {
        println!("{}. {:?} — {}", index + 1, slot.phase, slot.title);
    }
    Ok(())
}

fn run_one_action(config: &Config, target: TargetId, action: &str) -> Result<()> {
    if let Some(command) = TargetCommand::parse(action) {
        let resolved = command.resolve(target, &config.target_order)?;
        println!("{}", resolved);
        return Ok(());
    }
    let slots = if target == TargetId::Codex && action.starts_with("thread.jump.") {
        CodexState::open(config)?.slots()?.slots
    } else {
        Vec::new()
    };
    actions::run(target, action, app_name(config, target), &slots)
}

fn record_codex_hook() -> Result<()> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("failed to read Codex hook event from stdin")?;
    codex::record_hook_event(&input)?;
    Ok(())
}

fn pair_t3(pairing_url: Option<&str>) -> Result<()> {
    let entered;
    let pairing_url = match pairing_url {
        Some(url) => url,
        None => {
            entered = rpassword::prompt_password("Paste the T3 pairing URL: ")
                .context("failed to read pairing URL")?;
            entered.trim()
        }
    };
    let origin = t3::pair(pairing_url)?;
    println!("Paired with {origin}; the read-only credential is stored in Keychain.");
    Ok(())
}

fn unpair_t3() -> Result<()> {
    if t3::unpair()? {
        println!("Removed the saved T3 API credential from Keychain.");
    } else {
        println!("No saved T3 API credential was found.");
    }
    Ok(())
}

#[derive(Default)]
struct TargetRuntime {
    slots: Vec<ThreadSlot>,
    source: Option<StateSource>,
    degraded_reason: Option<String>,
    last_error_at: Option<Instant>,
}

#[derive(Clone)]
struct ResolvedKeyBinding {
    target: TargetId,
    map_name: String,
    tap_action: String,
    hold_action: Option<String>,
    double_tap_action: Option<String>,
    actuation_threshold: f32,
    release_threshold: f32,
    is_combo: bool,
}

fn run(config: Config) -> Result<()> {
    let api = HidApi::new().context("failed to initialize HID access")?;
    let mut input = UwUInput::open(&api)?;
    let mut rgb = UwURgb::open(&api)?;
    let mut active_target = config.default_target;
    let mut active_layers = HashMap::new();
    for target in &config.target_order {
        active_layers.insert(*target, 0);
    }
    let mut controller = LayerController::new(0);
    let mut pending_target = None;
    let mut target_switched_at = Some(Instant::now());
    let mut hall_triggers = [Trigger::default(); 3];
    let mut key_gestures: [KeyGestureController; 3] =
        std::array::from_fn(|_| KeyGestureController::default());
    let mut key_bindings: [Option<ResolvedKeyBinding>; 3] = std::array::from_fn(|_| None);
    let mut button_triggers = [Trigger::default(); 3];
    let mut runtimes = HashMap::<TargetId, TargetRuntime>::new();
    let mut next_render = Instant::now();
    let animation_start = Instant::now();
    let running = Arc::new(AtomicBool::new(true));
    let running_for_signal = Arc::clone(&running);
    ctrlc::set_handler(move || running_for_signal.store(false, Ordering::SeqCst))
        .context("failed to install Ctrl-C handler")?;
    let state_updates = spawn_state_workers(config.clone(), Arc::clone(&running));

    eprintln!(
        "uwu-vibe connected — target {} — layer 1: {}",
        active_target.label(),
        config.target(active_target).layers[0].name
    );
    eprintln!("Tap a button to select its layer; hold it to arm three combo actions.");
    eprintln!("Double-tap the middle button to cycle targets.");
    eprintln!("Hold Tools and press the right HE key to cycle targets.");
    eprintln!("The active Wootility profile should leave all six controls unbound.");
    warn_if_wootility_is_running();

    while running.load(Ordering::SeqCst) {
        let samples = input.read(20)?;
        let now = Instant::now();
        for (index, trigger) in button_triggers.iter_mut().enumerate() {
            let value = sample_value(samples, config.layer_buttons[index]);
            match trigger.transition(value, config.actuation_threshold, config.release_threshold) {
                Some(TriggerTransition::Pressed) => {
                    let double_tap_enabled = configured_action(
                        config.target(active_target).layers[index]
                            .double_tap_action
                            .as_deref(),
                    )
                    .is_some();
                    if let Some(event) = controller.button_pressed(
                        index,
                        now,
                        Duration::from_millis(config.double_tap_ms),
                        double_tap_enabled,
                    ) {
                        if handle_controller_event(
                            event,
                            &config,
                            &mut active_target,
                            &mut active_layers,
                            &mut controller,
                            &runtimes,
                        )? {
                            target_switched_at = Some(now);
                        }
                        next_render = now;
                    }
                }
                Some(TriggerTransition::Released) => {
                    let was_armed = controller.combo_layer().is_some();
                    if let Some(event) = controller.button_released(index, now) {
                        if handle_controller_event(
                            event,
                            &config,
                            &mut active_target,
                            &mut active_layers,
                            &mut controller,
                            &runtimes,
                        )? {
                            target_switched_at = Some(now);
                        }
                        next_render = now;
                    } else if was_armed {
                        next_render = now;
                    }
                    if !controller.gesture_active()
                        && let Some(command) = pending_target.take()
                    {
                        switch_target(
                            command,
                            &config,
                            &mut active_target,
                            &mut active_layers,
                            &mut controller,
                        )?;
                        target_switched_at = Some(now);
                        next_render = now;
                    }
                }
                None => {}
            }
        }
        if let Some(event) = controller.update(
            now,
            Duration::from_millis(config.combo_hold_ms),
            Duration::from_millis(config.double_tap_ms),
        ) {
            if handle_controller_event(
                event,
                &config,
                &mut active_target,
                &mut active_layers,
                &mut controller,
                &runtimes,
            )? {
                target_switched_at = Some(now);
            }
            next_render = now;
        }
        for index in 0..hall_triggers.len() {
            let value = sample_value(samples, config.hall_keys[index]);
            let (actuation_threshold, release_threshold) =
                key_bindings[index].as_ref().map_or_else(
                    || current_key_thresholds(&config, active_target, &controller, index),
                    |binding| (binding.actuation_threshold, binding.release_threshold),
                );
            match hall_triggers[index].transition(value, actuation_threshold, release_threshold) {
                Some(TriggerTransition::Pressed) => {
                    if key_bindings[index].is_none() {
                        key_bindings[index] =
                            resolve_key_binding(&config, active_target, &mut controller, index);
                    }
                    let Some(binding) = key_bindings[index].as_ref() else {
                        eprintln!("combo not armed yet, or a target switch is pending");
                        continue;
                    };
                    if let Some(event) = key_gestures[index].pressed(
                        now,
                        binding.hold_action.is_some(),
                        binding.double_tap_action.is_some(),
                        Duration::from_millis(config.double_tap_ms),
                    ) {
                        let binding = binding.clone();
                        if dispatch_key_gesture(
                            event,
                            index,
                            &binding,
                            &config,
                            &mut active_target,
                            &mut active_layers,
                            &mut controller,
                            &mut pending_target,
                            &runtimes,
                        )? {
                            target_switched_at = Some(now);
                        }
                    }
                }
                Some(TriggerTransition::Released) => {
                    if let Some(event) = key_gestures[index].released(now)
                        && let Some(binding) = key_bindings[index].clone()
                        && dispatch_key_gesture(
                            event,
                            index,
                            &binding,
                            &config,
                            &mut active_target,
                            &mut active_layers,
                            &mut controller,
                            &mut pending_target,
                            &runtimes,
                        )?
                    {
                        target_switched_at = Some(now);
                    }
                    if key_gestures[index].is_idle() {
                        key_bindings[index] = None;
                    }
                }
                None => {}
            }
        }
        for index in 0..key_gestures.len() {
            if let Some(event) = key_gestures[index].update(
                now,
                Duration::from_millis(config.key_hold_ms),
                Duration::from_millis(config.double_tap_ms),
            ) && let Some(binding) = key_bindings[index].clone()
                && dispatch_key_gesture(
                    event,
                    index,
                    &binding,
                    &config,
                    &mut active_target,
                    &mut active_layers,
                    &mut controller,
                    &mut pending_target,
                    &runtimes,
                )?
            {
                target_switched_at = Some(now);
            }
            if key_gestures[index].is_idle() {
                key_bindings[index] = None;
            }
        }

        while let Ok(update) = state_updates.try_recv() {
            let target = update.target();
            let runtime = runtimes.entry(target).or_default();
            match update {
                StateUpdate::Snapshot { snapshot, .. } => {
                    if runtime.source != Some(snapshot.source) {
                        eprintln!(
                            "{} state source: {}",
                            target.label(),
                            snapshot.source.label()
                        );
                        runtime.source = Some(snapshot.source);
                    }
                    if snapshot.degraded_reason != runtime.degraded_reason {
                        if let Some(reason) = &snapshot.degraded_reason {
                            eprintln!("{} state fallback: {reason}", target.label());
                        } else if runtime.degraded_reason.is_some() {
                            eprintln!("{} primary state source restored", target.label());
                        }
                        runtime.degraded_reason = snapshot.degraded_reason;
                    }
                    runtime.slots = snapshot.slots;
                    runtime.last_error_at = None;
                    if target == active_target {
                        next_render = now;
                    }
                }
                StateUpdate::Error { error, .. } => {
                    let should_report = runtime.last_error_at.is_none_or(|reported| {
                        now.duration_since(reported) >= Duration::from_secs(10)
                    });
                    if should_report {
                        eprintln!("{} state error: {error}", target.label());
                        runtime.last_error_at = Some(now);
                    }
                }
            }
        }
        if now >= next_render {
            if !running.load(Ordering::SeqCst) {
                continue;
            }
            let slots = runtimes
                .get(&active_target)
                .map_or(&[][..], |runtime| runtime.slots.as_slice());
            let switch_elapsed = target_switched_at.map(|switched| now.duration_since(switched));
            let frame = render_frame(
                &config,
                active_target,
                controller.active_layer(),
                controller.combo_layer(),
                slots,
                animation_start.elapsed(),
                switch_elapsed,
            );
            if let Err(error) = rgb.set_frame(&frame)
                && running.load(Ordering::SeqCst)
            {
                eprintln!("RGB error: {error:#}");
            }
            next_render = now + Duration::from_millis(100);
        }
    }
    eprintln!("restoring onboard UwU lighting");
    rgb.reset()
}

fn switch_target(
    command: TargetCommand,
    config: &Config,
    active_target: &mut TargetId,
    active_layers: &mut HashMap<TargetId, usize>,
    controller: &mut LayerController,
) -> Result<()> {
    let next = command.resolve(*active_target, &config.target_order)?;
    if next == *active_target {
        return Ok(());
    }
    active_layers.insert(*active_target, controller.active_layer());
    let next_layer = active_layers.get(&next).copied().unwrap_or(0);
    *controller = LayerController::new(next_layer);
    *active_target = next;
    eprintln!(
        "target {} — layer {}: {}",
        next.label(),
        next_layer + 1,
        config.target(next).layers[next_layer].name
    );
    Ok(())
}

fn handle_controller_event(
    event: ControllerEvent,
    config: &Config,
    active_target: &mut TargetId,
    active_layers: &mut HashMap<TargetId, usize>,
    controller: &mut LayerController,
    runtimes: &HashMap<TargetId, TargetRuntime>,
) -> Result<bool> {
    match event {
        ControllerEvent::LayerSelected(layer) => {
            eprintln!(
                "{} layer {}: {}",
                active_target,
                layer + 1,
                config.target(*active_target).layers[layer].name
            );
        }
        ControllerEvent::ComboArmed(layer) => {
            let layer_config = &config.target(*active_target).layers[layer];
            eprintln!(
                "{} hold layer {} armed: {}",
                active_target,
                layer + 1,
                layer_config.hold.name
            );
        }
        ControllerEvent::DoubleTapped(layer) => {
            let Some(action) = configured_action(
                config.target(*active_target).layers[layer]
                    .double_tap_action
                    .as_deref(),
            ) else {
                return Ok(false);
            };
            let action = action.to_owned();
            eprintln!(
                "{} button {} double-tap -> {}",
                active_target,
                layer + 1,
                action
            );
            if let Some(command) = TargetCommand::parse(&action) {
                let previous = *active_target;
                switch_target(command, config, active_target, active_layers, controller)?;
                return Ok(previous != *active_target);
            }
            let slots = runtimes
                .get(active_target)
                .map_or(&[][..], |runtime| runtime.slots.as_slice());
            if let Err(error) = actions::run(
                *active_target,
                &action,
                app_name(config, *active_target),
                slots,
            ) {
                eprintln!("double-tap action error: {error:#}");
            }
        }
    }
    Ok(false)
}

fn current_key_thresholds(
    config: &Config,
    target: TargetId,
    controller: &LayerController,
    key: usize,
) -> (f32, f32) {
    let target_config = config.target(target);
    let gesture = if let Some(layer) = controller.combo_layer() {
        &target_config.layers[layer].hold.key_gestures[key]
    } else {
        &target_config.layers[controller.active_layer()].key_gestures[key]
    };
    (
        gesture
            .actuation_threshold
            .unwrap_or(config.actuation_threshold),
        gesture
            .release_threshold
            .unwrap_or(config.release_threshold),
    )
}

fn resolve_key_binding(
    config: &Config,
    target: TargetId,
    controller: &mut LayerController,
    key: usize,
) -> Option<ResolvedKeyBinding> {
    let target_config = config.target(target);
    let (map_name, tap_action, gesture, is_combo) = match controller.key_pressed(key) {
        KeyRoute::Base { layer, key } => {
            let map = &target_config.layers[layer];
            (
                map.name.clone(),
                map.actions[key].clone(),
                &map.key_gestures[key],
                false,
            )
        }
        KeyRoute::Combo { layer, key } => {
            let map = &target_config.layers[layer].hold;
            (
                map.name.clone(),
                map.actions[key].clone(),
                &map.key_gestures[key],
                true,
            )
        }
        KeyRoute::Suppressed => return None,
    };
    Some(ResolvedKeyBinding {
        target,
        map_name,
        tap_action,
        hold_action: configured_action(gesture.hold_action.as_deref()).map(str::to_owned),
        double_tap_action: configured_action(gesture.double_tap_action.as_deref())
            .map(str::to_owned),
        actuation_threshold: gesture
            .actuation_threshold
            .unwrap_or(config.actuation_threshold),
        release_threshold: gesture
            .release_threshold
            .unwrap_or(config.release_threshold),
        is_combo,
    })
}

#[allow(clippy::too_many_arguments)]
fn dispatch_key_gesture(
    event: KeyGestureEvent,
    key: usize,
    binding: &ResolvedKeyBinding,
    config: &Config,
    active_target: &mut TargetId,
    active_layers: &mut HashMap<TargetId, usize>,
    controller: &mut LayerController,
    pending_target: &mut Option<TargetCommand>,
    runtimes: &HashMap<TargetId, TargetRuntime>,
) -> Result<bool> {
    let (gesture_name, action) = match event {
        KeyGestureEvent::Tap => ("tap", Some(binding.tap_action.as_str())),
        KeyGestureEvent::Hold => ("hold", binding.hold_action.as_deref()),
        KeyGestureEvent::DoubleTap => ("double-tap", binding.double_tap_action.as_deref()),
    };
    let Some(action) = action else {
        return Ok(false);
    };
    eprintln!(
        "{} / {} / key {} {} -> {}",
        binding.target,
        binding.map_name,
        key + 1,
        gesture_name,
        action
    );
    if let Some(command) = TargetCommand::parse(action) {
        if binding.is_combo && controller.gesture_active() {
            *pending_target = Some(command);
            controller.suppress_keys_until_release();
            eprintln!("target switch queued until the layer button is released");
            return Ok(false);
        }
        let previous = *active_target;
        switch_target(command, config, active_target, active_layers, controller)?;
        return Ok(previous != *active_target);
    }
    let slots = runtimes
        .get(&binding.target)
        .map_or(&[][..], |runtime| runtime.slots.as_slice());
    if let Err(error) = actions::run(
        binding.target,
        action,
        app_name(config, binding.target),
        slots,
    ) {
        eprintln!("key gesture action error: {error:#}");
    }
    Ok(false)
}

fn configured_action(action: Option<&str>) -> Option<&str> {
    action
        .map(str::trim)
        .filter(|action| !action.is_empty() && *action != "none")
}

enum StateUpdate {
    Snapshot {
        target: TargetId,
        snapshot: StateSnapshot,
    },
    Error {
        target: TargetId,
        error: String,
    },
}

impl StateUpdate {
    const fn target(&self) -> TargetId {
        match self {
            Self::Snapshot { target, .. } | Self::Error { target, .. } => *target,
        }
    }
}

fn spawn_state_workers(config: Config, running: Arc<AtomicBool>) -> Receiver<StateUpdate> {
    let (sender, receiver) = mpsc::channel();
    for target in config.target_order.iter().copied() {
        match target {
            TargetId::T3 => {
                spawn_t3_state_worker(config.clone(), Arc::clone(&running), sender.clone())
            }
            TargetId::Codex => {
                spawn_codex_state_worker(config.clone(), Arc::clone(&running), sender.clone())
            }
        }
    }
    receiver
}

fn spawn_t3_state_worker(config: Config, running: Arc<AtomicBool>, sender: Sender<StateUpdate>) {
    thread::spawn(move || {
        let mut state = None;
        while running.load(Ordering::SeqCst) {
            if state.is_none() {
                match T3State::open(&config) {
                    Ok(opened) => state = Some(opened),
                    Err(error) => {
                        if send_state_error(&sender, TargetId::T3, &error).is_err() {
                            return;
                        }
                    }
                }
            }
            if let Some(opened) = &state {
                let update = match opened.slots() {
                    Ok(snapshot) => StateUpdate::Snapshot {
                        target: TargetId::T3,
                        snapshot,
                    },
                    Err(error) => StateUpdate::Error {
                        target: TargetId::T3,
                        error: format!("{error:#}"),
                    },
                };
                if sender.send(update).is_err() {
                    return;
                }
            }
            thread::sleep(Duration::from_millis(config.poll_interval_ms));
        }
    });
}

fn spawn_codex_state_worker(config: Config, running: Arc<AtomicBool>, sender: Sender<StateUpdate>) {
    thread::spawn(move || {
        let mut state = None;
        while running.load(Ordering::SeqCst) {
            if state.is_none() {
                match CodexState::open(&config) {
                    Ok(opened) => state = Some(opened),
                    Err(error) => {
                        if send_state_error(&sender, TargetId::Codex, &error).is_err() {
                            return;
                        }
                    }
                }
            }
            if let Some(opened) = state.as_mut() {
                match opened.slots() {
                    Ok(snapshot) => {
                        if sender
                            .send(StateUpdate::Snapshot {
                                target: TargetId::Codex,
                                snapshot,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        if send_state_error(&sender, TargetId::Codex, &error).is_err() {
                            return;
                        }
                        state = None;
                    }
                }
            }
            thread::sleep(Duration::from_millis(config.poll_interval_ms));
        }
    });
}

fn send_state_error(
    sender: &Sender<StateUpdate>,
    target: TargetId,
    error: &anyhow::Error,
) -> std::result::Result<(), mpsc::SendError<StateUpdate>> {
    sender.send(StateUpdate::Error {
        target,
        error: format!("{error:#}"),
    })
}

fn warn_if_wootility_is_running() {
    let running = std::process::Command::new("pgrep")
        .args(["-x", "Wootility"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if running {
        eprintln!(
            "warning: Wootility is running and may override uwu-vibe lighting; save the device profile, then quit Wootility"
        );
    }
}

fn app_name(config: &Config, target: TargetId) -> &str {
    match target {
        TargetId::T3 => &config.t3_app_name_contains,
        TargetId::Codex => &config.codex_app_name_contains,
    }
}

fn sample_value(samples: &HashMap<Position, hardware::KeySample>, position: Position) -> f32 {
    samples.get(&position).map_or(0.0, |sample| sample.value)
}

fn render_frame(
    config: &Config,
    target: TargetId,
    active_layer: usize,
    combo_layer: Option<usize>,
    slots: &[ThreadSlot],
    elapsed: Duration,
    switch_elapsed: Option<Duration>,
) -> HashMap<Position, Rgb> {
    let target_config = config.target(target);
    if switch_elapsed.is_some_and(|elapsed| elapsed < Duration::from_millis(650)) {
        let accent = Rgb::from_hex(&target_config.accent).unwrap_or(Rgb(120, 100, 255));
        let intensity = 0.55 + 0.25 * ((elapsed.as_secs_f32() * 10.0).sin() + 1.0) / 2.0;
        return LED_POSITIONS
            .into_iter()
            .map(|position| (position, accent.scale(config.brightness * intensity)))
            .collect();
    }

    let visual_layer = combo_layer.unwrap_or(active_layer);
    let layer = &target_config.layers[visual_layer];
    let color = if combo_layer.is_some() {
        &layer.hold.color
    } else {
        &layer.color
    };
    let layer_color = Rgb::from_hex(color).unwrap_or(Rgb(120, 100, 255));
    let pulse = 0.28 + 0.14 * ((elapsed.as_secs_f32() * 2.4).sin() + 1.0);
    let mut frame = EDGE_POSITIONS
        .into_iter()
        .map(|position| (position, layer_color.scale(config.brightness * pulse)))
        .collect::<HashMap<_, _>>();

    let accent = Rgb::from_hex(&target_config.accent).unwrap_or_default();
    for position in [
        Position::new(0, 0),
        Position::new(0, 2),
        Position::new(0, 4),
        Position::new(0, 6),
    ] {
        frame.insert(position, accent.scale(config.brightness * 0.42));
    }

    for (index, position) in config.layer_buttons.iter().copied().enumerate() {
        let color =
            if combo_layer == Some(index) || (combo_layer.is_none() && index == active_layer) {
                Rgb(255, 255, 255)
            } else if combo_layer.is_some() && index == active_layer {
                Rgb::from_hex(&target_config.layers[index].color)
                    .unwrap_or_default()
                    .scale(0.45)
            } else {
                Rgb::from_hex(&target_config.layers[index].color)
                    .unwrap_or_default()
                    .scale(0.15)
            };
        frame.insert(position, color.scale(config.brightness));
    }

    for (index, position) in config.hall_keys.iter().copied().enumerate() {
        let color = if combo_layer.is_none() && active_layer == 0 {
            slots.get(index).map_or_else(
                || Rgb::from_hex(&target_config.status.unknown).unwrap_or_default(),
                |slot| {
                    Rgb::from_hex(target_config.status.color_for(slot.phase)).unwrap_or_default()
                },
            )
        } else {
            layer_color
        };
        frame.insert(position, color.scale(config.brightness));
    }
    frame
}
