//! `box`: see [`box_cli`]. Exit codes: 0 success, 1 failure (or `box check`
//! found errors), 2 usage error (error code `usage`, whether the command line
//! or an argument's value was wrong). Output cut short by a closed pipe is
//! not a failure.

use std::io::Write;
use std::process::ExitCode;

use box_cli::{Cli, CliError, run};
use clap::Parser;
use clap::error::ErrorKind;

fn init_logging() {
    use std::io::IsTerminal;
    use tracing_subscriber::filter::filter_fn;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{EnvFilter, Layer};
    // Logs go to stderr so stdout stays machine-readable. Scripts' `print`
    // and `log.info` (target `box_script`) show by default, as in the player.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,box_script=info"));
    // While a command collects problems into its result, the engine's
    // warnings and errors are reported there instead (once).
    let not_collected = filter_fn(|meta| {
        !(box_cli::collecting()
            && meta.target().starts_with("box")
            && *meta.level() <= tracing::Level::WARN)
    });
    // The capture ring feeds `problems` and `box run --control`'s `read_log`,
    // which keeps info records like the player's own logging.
    let capture = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(box_engine::CAPTURE_LOG_FILTER));
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(std::io::stderr().is_terminal())
                .with_filter(filter)
                .with_filter(not_collected),
        )
        .with(box_engine::log_capture::CaptureLayer.with_filter(capture))
        .try_init();
}

/// Print a line on stdout. A reader that went away (`box registry | head -1`)
/// is not an error; any other failure is reported and returns false.
fn print_line(text: &str) -> bool {
    let mut out = std::io::stdout().lock();
    match writeln!(out, "{text}").and_then(|()| out.flush()) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => true,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "error[io]: stdout: {e}");
            false
        }
    }
}

fn print_json(value: &serde_json::Value) -> bool {
    print_line(&serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()))
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let json = std::env::args().any(|a| a == "--json");
            if json && !matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
                let message = e.render().to_string();
                let message = message
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim_start_matches("error: ");
                print_json(&CliError::new("usage", message).to_json());
                return ExitCode::from(2);
            }
            let _ = e.print();
            return ExitCode::from(if e.use_stderr() { 2 } else { 0 });
        }
    };
    init_logging();
    box_engine::crash::install("cli");
    match run(&cli) {
        Ok(output) => {
            let printed = if cli.json {
                print_json(&output.json)
            } else {
                output.text.is_empty() || print_line(&output.text)
            };
            if output.success && printed {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            if cli.json {
                print_json(&e.to_json());
            } else {
                let _ = writeln!(std::io::stderr(), "{e}");
            }
            ExitCode::from(e.exit_code())
        }
    }
}
