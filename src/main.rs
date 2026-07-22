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
use t3::{AgentPhase, T3State, ThreadSlot};

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
    /// Print the three T3 thread slots and their resolved state.
    T3State,
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
        Some(Command::T3State) => print_t3_state(&config),
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

fn print_t3_state(config: &Config) -> Result<()> {
    let state = T3State::open(&config.t3_database)?;
    for (index, slot) in state.slots()?.iter().enumerate() {
        println!("{}. {:?} — {}", index + 1, slot.phase, slot.title);
    }
    Ok(())
}

fn run(config: Config) -> Result<()> {
    let api = HidApi::new().context("failed to initialize HID access")?;
    let mut input = UwUInput::open(&api)?;
    let mut rgb = UwURgb::open(&api)?;
    let t3 = T3State::open(&config.t3_database)?;
    let mut active_layer = 0_usize;
    let mut hall_triggers = [Trigger::default(); 3];
    let mut button_triggers = [Trigger::default(); 3];
    let mut last_slots = Vec::new();
    let mut next_state_poll = Instant::now();
    let mut next_render = Instant::now();
    let animation_start = Instant::now();
    let running = Arc::new(AtomicBool::new(true));
    let running_for_signal = Arc::clone(&running);
    ctrlc::set_handler(move || running_for_signal.store(false, Ordering::SeqCst))
        .context("failed to install Ctrl-C handler")?;

    eprintln!("t3-uwu connected — layer 1: {}", config.layers[0].name);
    eprintln!("Top buttons select layers; HE keys run the three actions in that layer.");
    eprintln!("The active Wootility profile should leave all six controls unbound.");

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
        if now >= next_state_poll {
            match t3.slots() {
                Ok(slots) => last_slots = slots,
                Err(error) => eprintln!("T3 state error: {error:#}"),
            }
            next_state_poll = now + Duration::from_millis(config.poll_interval_ms);
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
            if let Err(error) = rgb.set_frame(&frame) {
                eprintln!("RGB error: {error:#}");
            }
            next_render = now + Duration::from_millis(100);
        }
    }
    eprintln!("restoring onboard UwU lighting");
    rgb.reset()
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
