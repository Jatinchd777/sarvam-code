mod agent;
mod api;
mod app;
mod config;
mod markdown;
mod session;
mod tools;
mod ui;

use std::{
    io::{self, Stdout, Write},
    time::Duration,
};

use anyhow::{Context, Result};
use clap::Parser;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    crossterm::{
        cursor::{Hide, Show},
        event::{
            self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste,
            EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent,
            MouseEventKind,
        },
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    },
};
use tokio::sync::mpsc;

use crate::{
    agent::{AgentEvent, AgentHandle, Canceller, Command, Decision},
    app::App,
    config::{Config, Overrides},
};

#[derive(Parser, Debug)]
#[command(
    name = "sarvam-code",
    version,
    about = "A minimal Sarvam AI coding agent with a Ratatui TUI"
)]
struct Cli {
    /// Run one prompt headlessly and exit (no TUI).
    #[arg(short = 'p', long = "print", value_name = "PROMPT")]
    print: Option<String>,

    /// Model name (overrides SARVAM_MODEL and the config file).
    #[arg(long, value_name = "MODEL")]
    model: Option<String>,

    /// Chat-completions endpoint.
    #[arg(long = "api-url", value_name = "URL")]
    api_url: Option<String>,

    /// ask | read-only | yolo.
    #[arg(long = "permission-mode", value_name = "MODE")]
    permission_mode: Option<String>,

    /// Reasoning effort: low | high | max (empty string omits the field).
    #[arg(long, value_name = "EFFORT")]
    effort: Option<String>,

    /// Maximum tool/model round-trips per user turn.
    #[arg(long = "max-steps", value_name = "N")]
    max_steps: Option<u32>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load(Overrides {
        model: cli.model,
        api_url: cli.api_url,
        effort: cli.effort,
        permission_mode: cli.permission_mode,
        max_steps: cli.max_steps,
    })?;

    let system_prompt = cfg.system_prompt();
    let mut handle = agent::spawn(cfg.clone(), system_prompt)?;

    if let Some(prompt) = cli.print {
        let result = run_print(&mut handle, prompt).await;
        let _ = handle.commands.send(Command::Shutdown).await;
        result
    } else {
        run_tui(&mut handle, &cfg).await
    }
}

async fn run_print(handle: &mut AgentHandle, prompt: String) -> Result<()> {
    handle
        .commands
        .send(Command::Prompt(prompt))
        .await
        .context("the agent task stopped")?;

    let mut failed = false;
    let mut warned = false;
    let mut stdout = io::stdout();

    while let Some(event) = handle.events.recv().await {
        match event {
            AgentEvent::TextDelta(text) => {
                print!("{text}");
                let _ = stdout.flush();
            }
            AgentEvent::ToolStarted { name, args, .. } => {
                eprintln!("\n→ {name} {}", one_line(&args, 200));
            }
            AgentEvent::ToolFinished { ok, output, .. } => {
                eprintln!(
                    "← {} ({} chars)",
                    if ok { "ok" } else { "error" },
                    output.chars().count()
                );
            }
            AgentEvent::PermissionRequest { id, summary, .. } => {
                if !warned {
                    warned = true;
                    eprintln!(
                        "\nheadless mode cannot ask for approval, denying.\n\
                         Set PERMISSION_MODE=yolo to allow mutating tools."
                    );
                }
                eprintln!("⛔ {summary}");
                let _ = handle.permissions.send((id, Decision::Deny)).await;
            }
            AgentEvent::Notice(text) => eprintln!("· {text}"),
            AgentEvent::TurnFinished => {
                println!();
                break;
            }
            AgentEvent::TurnFailed(error) => {
                println!();
                eprintln!("error: {error}");
                failed = true;
                break;
            }
            _ => {}
        }
    }

    if failed {
        std::process::exit(1);
    }
    Ok(())
}

async fn run_tui(handle: &mut AgentHandle, cfg: &Config) -> Result<()> {
    let _guard = TerminalGuard::install();
    let mut terminal = init_terminal()?;
    let mut app = App::new(cfg);

    let (input_tx, mut input_rx) = mpsc::channel::<Event>(64);
    std::thread::spawn(move || {
        loop {
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => match event::read() {
                    Ok(event) => {
                        if input_tx.blocking_send(event).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                Ok(false) => {}
                Err(_) => break,
            }
        }
    });

    let commands = handle.commands.clone();
    let permissions = handle.permissions.clone();
    let cancel = handle.cancel.clone();
    let events = &mut handle.events;

    let mut ticker = tokio::time::interval(Duration::from_millis(120));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        tokio::select! {
            _ = ticker.tick() => {
                app.spinner = app.spinner.wrapping_add(1);
            }
            Some(event) = input_rx.recv() => {
                handle_input(event, &mut app, &commands, &permissions, &cancel).await;
            }
            Some(event) = events.recv() => {
                app.on_agent_event(event);
            }
        }
        if app.quit {
            break;
        }
    }

    let _ = commands.send(Command::Shutdown).await;
    drop(terminal);
    Ok(())
}

async fn handle_input(
    event: Event,
    app: &mut App,
    commands: &mpsc::Sender<Command>,
    permissions: &mpsc::Sender<(u64, Decision)>,
    cancel: &Canceller,
) {
    match event {
        Event::Key(key) => handle_key(key, app, commands, permissions, cancel).await,
        Event::Mouse(mouse) => handle_mouse(mouse, app),
        Event::Paste(text) if !app.busy && app.permission.is_none() => {
            app.input.insert_str(&text);
            app.reset_completion();
        }
        _ => {}
    }
}

/// Mouse wheel scrolls the conversation by three lines per notch.
fn handle_mouse(mouse: MouseEvent, app: &mut App) {
    match mouse.kind {
        MouseEventKind::ScrollUp => app.scroll_up(3),
        MouseEventKind::ScrollDown => app.scroll_down(3),
        _ => {}
    }
}

async fn handle_key(
    key: KeyEvent,
    app: &mut App,
    commands: &mpsc::Sender<Command>,
    permissions: &mpsc::Sender<(u64, Decision)>,
    cancel: &Canceller,
) {
    if key.kind == KeyEventKind::Release {
        return;
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let page = app.viewport_height.saturating_sub(2).max(1);

    if ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')) {
        app.quit = true;
        return;
    }

    if let Some(modal) = app.permission.clone() {
        let decision = match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => Some(Decision::Allow),
            KeyCode::Char('a') | KeyCode::Char('A') => Some(Decision::Always),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Some(Decision::Deny),
            _ => None,
        };
        if let Some(decision) = decision {
            let _ = permissions.send((modal.id, decision)).await;
            app.permission = None;
            app.status = Some(
                match decision {
                    Decision::Allow => "allowed",
                    Decision::Always => "allowed for this session",
                    Decision::Deny => "denied",
                }
                .to_string(),
            );
        }
        return;
    }

    if app.busy {
        match key.code {
            KeyCode::Esc => {
                app.status = Some("cancelling…".to_string());
                cancel.cancel();
            }
            KeyCode::PageUp => app.scroll_up(page),
            KeyCode::PageDown => app.scroll_down(page),
            KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => app.scroll_up(3),
            KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => app.scroll_down(3),
            KeyCode::Home if ctrl => app.scroll_to_top(),
            KeyCode::End if ctrl => app.scroll_to_bottom(),
            KeyCode::Char('o') | KeyCode::Char('O') if ctrl => app.toggle_expand(),
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Enter => {
            let selected = app.selected_completion();
            if selected.is_some_and(|completion| completion.is_model()) {
                app.complete_selected();
                if let Some(command) = app.submit()
                    && commands.send(command).await.is_err()
                {
                    app.busy = false;
                    app.error("the agent task has stopped");
                }
            } else if selected.is_some_and(|completion| {
                matches!(completion, app::Completion::Command(command) if !command.args.is_empty())
                    || !completion.is_named(app.input.text.trim())
            }) {
                // Complete partial commands, and leave room for arguments.
                app.complete_selected();
            } else if let Some(command) = app.submit()
                && commands.send(command).await.is_err()
            {
                app.busy = false;
                app.error("the agent task has stopped");
            }
        }
        KeyCode::Esc => {
            if app.input.text.is_empty() {
                app.scroll_to_bottom();
            } else {
                app.input.clear();
                app.reset_completion();
            }
        }
        KeyCode::Backspace => {
            app.input.backspace();
            app.reset_completion();
        }
        KeyCode::Delete => {
            app.input.delete();
            app.reset_completion();
        }
        KeyCode::Left => app.input.left(),
        KeyCode::Right => app.input.right(),
        KeyCode::Home if ctrl => app.scroll_to_top(),
        KeyCode::End if ctrl => app.scroll_to_bottom(),
        KeyCode::Home => app.input.home(),
        KeyCode::End => app.input.end(),
        KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => app.scroll_up(3),
        KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => app.scroll_down(3),
        KeyCode::Up if app.completion_open() => app.completion_move(-1),
        KeyCode::Down if app.completion_open() => app.completion_move(1),
        KeyCode::Up => {
            app.input.history_prev();
            app.reset_completion();
        }
        KeyCode::Down => {
            app.input.history_next();
            app.reset_completion();
        }
        KeyCode::PageUp => app.scroll_up(page),
        KeyCode::PageDown => app.scroll_down(page),
        KeyCode::Tab => {
            if !app.complete_selected() {
                app.input.insert_str("  ");
            }
        }
        KeyCode::Char('o') | KeyCode::Char('O') if ctrl => app.toggle_expand(),
        KeyCode::Char('w') | KeyCode::Char('W') if ctrl => {
            app.input.kill_word();
            app.reset_completion();
        }
        KeyCode::Char('u') | KeyCode::Char('U') if ctrl => {
            app.input.clear();
            app.reset_completion();
        }
        KeyCode::Char('d') | KeyCode::Char('D') if ctrl => {
            if app.input.text.is_empty() {
                app.quit = true;
            }
        }
        KeyCode::Char(character) if !ctrl && !alt => {
            app.reset_completion();
            app.input.insert_char(character);
        }
        _ => {}
    }
}

fn one_line(text: &str, max: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| {
            if matches!(c, '\n' | '\r' | '\t') {
                ' '
            } else {
                c
            }
        })
        .collect();
    if flat.chars().count() <= max {
        flat
    } else {
        flat.chars().take(max).collect::<String>() + "…"
    }
}

struct TerminalGuard;

impl TerminalGuard {
    fn install() -> Self {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            previous(info);
        }));
        Self
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn init_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode().context("cannot enable raw mode")?;
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableMouseCapture,
        Hide
    )
    .context("cannot initialize the terminal")?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).context("cannot create the terminal")?;
    terminal.clear()?;
    Ok(terminal)
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        LeaveAlternateScreen,
        DisableBracketedPaste,
        DisableMouseCapture,
        Show
    );
}
