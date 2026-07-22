mod actions;
mod config;
mod hardware;
mod rgb;
mod t3;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use hidapi::HidApi;

use config::Config;
use hardware::{Position, Trigger, UwUInput};
use rgb::{EDGE_POSITIONS, LED_POSITIONS, Rgb, UwURgb};
use t3::{AgentPhase, StateSnapshot, T3State, ThreadSlot};

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
    /// Print the three T3 thread slots and their resolved state.
    T3State,
    /// Exchange a T3 pairing link for read-only API access and save it in Keychain.
    Pair {
        /// Pairing URL from T3. Omit it to enter the URL without shell-history exposure.
        pairing_url: Option<String>,
    },
    /// Remove t3-uwu's saved T3 API credential from Keychain.
    Unpair,
    /// Send one supported action to T3 Code (useful for permission testing).
    Action {
        /// For example: thread.jump.1, chat.new, or terminal.toggle.
        action: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load(cli.config.as_deref())?;
    config.validate()?;
    match cli.command {
        Some(Command::Diagnose { watch, raw }) => diagnose(&config, watch, raw),
        Some(Command::TestRgb) => test_rgb(&config),
        Some(Command::ResetRgb) => reset_rgb(),
        Some(Command::T3State) => print_t3_state(&config),
        Some(Command::Pair { pairing_url }) => pair_t3(pairing_url.as_deref()),
        Some(Command::Unpair) => unpair_t3(),
        Some(Command::Action { action }) => actions::run(&action, &config.t3_app_name_contains),
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

fn print_t3_state(config: &Config) -> Result<()> {
    let state = T3State::open(config)?;
    let snapshot = state.slots()?;
    println!("State source: {}", snapshot.source.label());
    if let Some(reason) = snapshot.degraded_reason {
        println!("API fallback reason: {reason}");
    }
    for (index, slot) in snapshot.slots.iter().enumerate() {
        println!("{}. {:?} — {}", index + 1, slot.phase, slot.title);
    }
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

fn run(config: Config) -> Result<()> {
    let api = HidApi::new().context("failed to initialize HID access")?;
    let mut input = UwUInput::open(&api)?;
    let mut rgb = UwURgb::open(&api)?;
    let mut active_layer = 0_usize;
    let mut hall_triggers = [Trigger::default(); 3];
    let mut button_triggers = [Trigger::default(); 3];
    let mut last_slots = Vec::new();
    let mut last_state_source = None;
    let mut last_degraded_reason: Option<String> = None;
    let mut last_state_error_at: Option<Instant> = None;
    let mut next_render = Instant::now();
    let animation_start = Instant::now();
    let running = Arc::new(AtomicBool::new(true));
    let running_for_signal = Arc::clone(&running);
    ctrlc::set_handler(move || running_for_signal.store(false, Ordering::SeqCst))
        .context("failed to install Ctrl-C handler")?;
    let state_updates = spawn_t3_state_worker(config.clone(), Arc::clone(&running));

    eprintln!("t3-uwu connected — layer 1: {}", config.layers[0].name);
    eprintln!("Top buttons select layers; HE keys run the three actions in that layer.");
    eprintln!("The active Wootility profile should leave all six controls unbound.");
    warn_if_wootility_is_running();

    while running.load(Ordering::SeqCst) {
        let samples = input.read(20)?;
        for (index, trigger) in button_triggers.iter_mut().enumerate() {
            let value = sample_value(samples, config.layer_buttons[index]);
            if trigger.update(value, config.actuation_threshold, config.release_threshold) {
                active_layer = index;
                eprintln!("layer {}: {}", index + 1, config.layers[index].name);
                next_render = Instant::now();
            }
        }
        for (index, trigger) in hall_triggers.iter_mut().enumerate() {
            let value = sample_value(samples, config.hall_keys[index]);
            if trigger.update(value, config.actuation_threshold, config.release_threshold) {
                let action = &config.layers[active_layer].actions[index];
                eprintln!(
                    "{} / key {} -> {}",
                    config.layers[active_layer].name,
                    index + 1,
                    action
                );
                if let Err(error) = actions::run(action, &config.t3_app_name_contains) {
                    eprintln!("action error: {error:#}");
                }
            }
        }

        let now = Instant::now();
        while let Ok(update) = state_updates.try_recv() {
            match update {
                StateUpdate::Snapshot(snapshot) => {
                    if last_state_source != Some(snapshot.source) {
                        eprintln!("T3 state source: {}", snapshot.source.label());
                        last_state_source = Some(snapshot.source);
                    }
                    if snapshot.degraded_reason != last_degraded_reason {
                        if let Some(reason) = &snapshot.degraded_reason {
                            eprintln!("T3 API unavailable; using SQLite: {reason}");
                        } else if last_degraded_reason.is_some() {
                            eprintln!("T3 API connection restored");
                        }
                        last_degraded_reason = snapshot.degraded_reason;
                    }
                    last_slots = snapshot.slots;
                    last_state_error_at = None;
                }
                StateUpdate::Error(error) => {
                    let should_report = last_state_error_at.is_none_or(|reported| {
                        now.duration_since(reported) >= Duration::from_secs(10)
                    });
                    if should_report {
                        eprintln!("T3 state error: {error}");
                        last_state_error_at = Some(now);
                    }
                }
            }
        }
        if now >= next_render {
            if !running.load(Ordering::SeqCst) {
                continue;
            }
            let frame = render_frame(
                &config,
                active_layer,
                &last_slots,
                animation_start.elapsed(),
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

enum StateUpdate {
    Snapshot(StateSnapshot),
    Error(String),
}

fn spawn_t3_state_worker(config: Config, running: Arc<AtomicBool>) -> Receiver<StateUpdate> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let state = match T3State::open(&config) {
            Ok(state) => state,
            Err(error) => {
                let _ = sender.send(StateUpdate::Error(format!("{error:#}")));
                return;
            }
        };
        while running.load(Ordering::SeqCst) {
            let update = match state.slots() {
                Ok(snapshot) => StateUpdate::Snapshot(snapshot),
                Err(error) => StateUpdate::Error(format!("{error:#}")),
            };
            if sender.send(update).is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(config.poll_interval_ms));
        }
    });
    receiver
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
            "warning: Wootility is running and may override t3-uwu lighting; save the device profile, then quit Wootility"
        );
    }
}

fn sample_value(samples: &HashMap<Position, hardware::KeySample>, position: Position) -> f32 {
    samples.get(&position).map_or(0.0, |sample| sample.value)
}

fn render_frame(
    config: &Config,
    active_layer: usize,
    slots: &[ThreadSlot],
    elapsed: Duration,
) -> HashMap<Position, Rgb> {
    let layer_color =
        Rgb::from_hex(&config.layers[active_layer].color).unwrap_or(Rgb(120, 100, 255));
    let pulse = 0.28 + 0.14 * ((elapsed.as_secs_f32() * 2.4).sin() + 1.0);
    let mut frame = EDGE_POSITIONS
        .into_iter()
        .map(|position| (position, layer_color.scale(config.brightness * pulse)))
        .collect::<HashMap<_, _>>();

    for (index, position) in config.layer_buttons.iter().copied().enumerate() {
        let color = if index == active_layer {
            Rgb(255, 255, 255)
        } else {
            Rgb::from_hex(&config.layers[index].color)
                .unwrap_or_default()
                .scale(0.15)
        };
        frame.insert(position, color.scale(config.brightness));
    }

    for (index, position) in config.hall_keys.iter().copied().enumerate() {
        let color = if active_layer == 0 {
            slots
                .get(index)
                .map_or(Rgb(25, 25, 30), |slot| phase_color(slot.phase))
        } else {
            layer_color
        };
        frame.insert(position, color.scale(config.brightness));
    }
    frame
}

fn phase_color(phase: AgentPhase) -> Rgb {
    match phase {
        AgentPhase::Idle => Rgb(25, 25, 30),
        AgentPhase::Starting => Rgb(80, 120, 255),
        AgentPhase::Running => Rgb(40, 120, 255),
        AgentPhase::WaitingApproval => Rgb(255, 90, 50),
        AgentPhase::WaitingInput => Rgb(255, 190, 45),
        AgentPhase::Completed => Rgb(55, 220, 120),
        AgentPhase::Failed => Rgb(255, 35, 65),
    }
}
