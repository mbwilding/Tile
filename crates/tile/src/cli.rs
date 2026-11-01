//! The command-line side of `tile`: talking to a running Tile, checking a config and installing.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::{Subcommand, ValueEnum};
use tile_ipc::{Client, Event, Query, Request, Response};

use crate::install;

/// What `tile` does instead of running the window manager.
#[derive(Subcommand)]
pub enum Cmd {
    /// Run a dispatcher, e.g. `tile dispatch workspace 3`.
    Dispatch {
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Print information about the window manager.
    Query {
        what: What,
        /// Print raw JSON.
        #[arg(long, short)]
        json: bool,
    },
    /// Print an event line (JSON) whenever the workspace, active window or layout changes.
    Subscribe,
    /// Reload the config file.
    Reload,
    /// Check a config file (default: `--config`, else %APPDATA%\Tile\config.yml) and print
    /// what is wrong.
    Check {
        /// The file to check.
        file: Option<PathBuf>,
    },
    /// Print the config file's path: `--config` if given, else the default one.
    Config,
    /// Install Tile: copy it to C:\Program Files\Tile, run it as administrator at sign-in (so
    /// it manages admin windows and remaps keys in them), put `tile` on your PATH and start it
    /// now. Run again after a rebuild to update.
    Install,
    /// Stop Tile and undo `install`. Your config and logs are kept.
    Uninstall,
    /// Internal: the admin part of `install` / `uninstall`, started through UAC.
    #[command(hide = true)]
    Elevated {
        /// The pipe to report the result to.
        #[arg(long)]
        report: String,
        #[command(subcommand)]
        step: install::Step,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum What {
    Monitors,
    Workspaces,
    Windows,
    Activewindow,
}

impl From<What> for Query {
    fn from(what: What) -> Self {
        match what {
            What::Monitors => Self::Monitors,
            What::Workspaces => Self::Workspaces,
            What::Windows => Self::Windows,
            What::Activewindow => Self::ActiveWindow,
        }
    }
}

fn print_human(response: &Response) {
    match response {
        Response::Ok => {}
        Response::Error { message } => eprintln!("error: {message}"),
        Response::Monitors { monitors } => {
            for m in monitors {
                let focus = if m.focused { " (focused)" } else { "" };
                println!(
                    "monitor {}: {}x{} at {},{} workspace {}{focus}",
                    m.index, m.rect.w, m.rect.h, m.rect.x, m.rect.y, m.workspace
                );
            }
        }
        Response::Workspaces { workspaces } => {
            for w in workspaces {
                let monitor = w.monitor.map_or_else(|| "-".into(), |m| m.to_string());
                let flags = match (w.visible, w.focused) {
                    (_, true) => " (focused)",
                    (true, false) => " (visible)",
                    _ => "",
                };
                println!(
                    "workspace {}: monitor {monitor}, {} windows, {}{flags}",
                    w.id, w.windows, w.layout
                );
            }
        }
        Response::Windows { windows } => {
            for w in windows {
                let mut flags = Vec::new();
                for (on, name) in [
                    (w.focused, "focused"),
                    (w.floating, "floating"),
                    (w.fullscreen, "fullscreen"),
                    (w.hidden, "hidden"),
                ] {
                    if on {
                        flags.push(name);
                    }
                }
                println!(
                    "{:#x} [ws {}] {} — {} ({}) {}",
                    w.id,
                    w.workspace,
                    w.exe,
                    w.title,
                    w.class,
                    flags.join(",")
                );
            }
        }
        Response::ActiveWindow { window } => match window {
            Some(w) => println!("{:#x} [ws {}] {} — {}", w.id, w.workspace, w.exe, w.title),
            None => println!("no active window"),
        },
        Response::Event { event } => print_event(event),
    }
}

fn print_event(event: &Event) {
    println!("{}", serde_json::to_string(event).unwrap_or_default());
}

fn check(path: &Path) -> anyhow::Result<()> {
    tile_config::Loaded::load(path)?;
    println!("{}: ok", path.display());
    Ok(())
}

/// Runs `command`: a request to the running Tile, or one of the commands that work without it.
/// `config` is the `--config` file, if given.
pub async fn run(command: Cmd, config: Option<PathBuf>) -> anyhow::Result<()> {
    let config = config.unwrap_or_else(tile_config::default_path);
    let (request, json) = match command {
        Cmd::Dispatch { command } => (
            Request::Dispatch {
                command: command.join(" "),
            },
            false,
        ),
        Cmd::Query { what, json } => (Request::Query { what: what.into() }, json),
        Cmd::Subscribe => (Request::Subscribe, true),
        Cmd::Reload => (
            Request::Dispatch {
                command: "reload".into(),
            },
            false,
        ),
        Cmd::Check { file } => return check(file.as_deref().unwrap_or(&config)),
        Cmd::Config => {
            println!("{}", config.display());
            return Ok(());
        }
        Cmd::Install => return install::install().await,
        Cmd::Uninstall => return install::uninstall().await,
        Cmd::Elevated { report, step } => return install::elevated(step, &report).await,
    };
    let mut client = Client::send(&request).await.context("is Tile running?")?;
    let mut failed = false;
    while let Some(response) = client.next().await? {
        failed |= matches!(response, Response::Error { .. });
        match (&response, json) {
            (Response::Event { event }, _) => print_event(event),
            (Response::Ok, _) => {}
            (_, true) => println!("{}", serde_json::to_string_pretty(&response)?),
            (_, false) => print_human(&response),
        }
    }
    if failed {
        bail!("request failed");
    }
    Ok(())
}
