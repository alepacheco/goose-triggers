//! goose-triggers: rules that start headless goose recipe runs when something happens
//! (a new file in a folder, a webhook), and hold a run at the actions the user marks
//! until they approve or reject it.

mod commands;
mod config;
mod daemon;
mod folder;
mod hook;
mod hostile;
mod plugin;
mod recipe;
mod runner;
mod scripted_model;
mod selftest;
mod store;
mod util;
mod webhook;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "goose-triggers",
    version,
    about = "Event-driven background runs for goose, with approval holds"
)]
struct Cli {
    /// Config file [default: $GOOSE_TRIGGERS_CONFIG, else ~/.config/goose-triggers/config.yaml]
    #[arg(short, long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Watch the rules' folders and webhooks and run their goose recipes (runs until stopped)
    Run,
    /// List runs; held runs show the call they wait on
    Status {
        /// List every run, not only held runs and the 20 latest
        #[arg(long)]
        all: bool,
    },
    /// Show what happened in one run, or the latest events of all runs
    Log {
        /// The run's number, as `status` shows it
        run: Option<i64>,
    },
    /// Approve the call a held run waits on: the run resumes in the same goose session
    Approve {
        /// The run's number, as `status` shows it
        run: i64,
        /// Approve without asking (needed when not run from a terminal)
        #[arg(long, short)]
        yes: bool,
    },
    /// Reject the call a held run waits on: the run ends
    Reject {
        /// The run's number, as `status` shows it
        run: i64,
    },
    /// Run the whole path (event, run, hold, restart, approve, resume, done) against a scripted model server; no API key needed
    Selftest {
        /// The goose binary to test with [default: goose on PATH]
        #[arg(long, value_name = "PATH")]
        goose: Option<PathBuf>,
        /// Keep the selftest's temporary folder even when it passes
        #[arg(long)]
        keep: bool,
    },
    /// Run hostile events (injected instructions in webhook fields and files) against the example rules' recipes, with no holds and with their holds; by default a scripted model makes every call the injections name (no API key needed), or a real model through goose's own provider
    HostileSuite {
        /// The goose binary to test with [default: goose on PATH]
        #[arg(long, value_name = "PATH")]
        goose: Option<PathBuf>,
        /// The holds of the second pass, comma-separated [default: shell,write,edit,read_image, the example rules']
        #[arg(long, value_name = "TOOLS")]
        holds: Option<String>,
        /// Keep the suite's temporary folder even when every result is as expected
        #[arg(long)]
        keep: bool,
        /// Send the events to a real model through this goose provider (ollama, openai, anthropic, ...), set up through the environment as goose reads it (OLLAMA_HOST, OPENAI_API_KEY, ...); needs --model
        #[arg(long, value_name = "NAME", requires = "model")]
        provider: Option<String>,
        /// The real model, as the provider names it (qwen3:8b, ...); needs --provider
        #[arg(long, value_name = "NAME", requires = "provider")]
        model: Option<String>,
        /// With a real model: also send this many plain entries (at most 10) and approve every call they are held at, to see what the model does after an approval
        #[arg(long, value_name = "N", default_value_t = 0, requires = "provider")]
        approvals: usize,
        /// With a real model: how long one run may take, in minutes
        #[arg(long, value_name = "MINUTES", default_value_t = 20)]
        run_minutes: u64,
    },
    /// Called by goose through goose-triggers' plugin hooks; not for direct use
    #[command(hide = true)]
    Hook,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Cmd::Hook = cli.command {
        return ExitCode::from(hook::main_hook());
    }
    if let Cmd::Status { .. } | Cmd::Log { .. } = cli.command {
        // These only print, for a reader that may stop reading (`status | head`): end quietly
        // then, as other command-line tools do, rather than fail on the closed pipe. approve
        // and reject decide first and print on regardless.
        // SAFETY: restores the default action of one signal before any thread starts.
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        }
    }
    let result = match cli.command {
        Cmd::Run => config::Config::load(cli.config.as_deref()).and_then(daemon::run),
        Cmd::Status { all } => config::Config::load_for_state(cli.config.as_deref())
            .and_then(|c| commands::status(&c, all)),
        Cmd::Log { run } => config::Config::load_for_state(cli.config.as_deref())
            .and_then(|c| commands::log(&c, run)),
        Cmd::Approve { run, yes } => config::Config::load_for_state(cli.config.as_deref())
            .and_then(|c| commands::approve(&c, run, yes)),
        Cmd::Reject { run } => config::Config::load_for_state(cli.config.as_deref())
            .and_then(|c| commands::reject(&c, run)),
        Cmd::Selftest { goose, keep } => selftest::run(goose, keep),
        Cmd::HostileSuite {
            goose,
            holds,
            keep,
            provider,
            model,
            approvals,
            run_minutes,
        } => hostile::run(hostile::Options {
            goose,
            holds,
            keep,
            model: match (provider, model) {
                (Some(provider), Some(model)) => hostile::Model::Real { provider, model },
                _ => hostile::Model::Scripted,
            },
            approvals,
            run_minutes,
        }),
        Cmd::Hook => unreachable!(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("goose-triggers: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// The environment variable naming the config file.
pub const CONFIG_ENV: &str = "GOOSE_TRIGGERS_CONFIG";

/// The default config file, `~/.config/goose-triggers/config.yaml`, used when neither `-c`
/// nor `GOOSE_TRIGGERS_CONFIG` names one.
pub fn default_config_path() -> PathBuf {
    util::config_home()
        .join("goose-triggers")
        .join("config.yaml")
}
