use clap::{Parser, Subcommand};
use std::{
    process::{Command, ExitCode},
    thread,
    time::Duration,
};

const SERVICE_NAME: &str = "Hisoka";

#[derive(Parser)]
#[command(version, about = "Hisoka service management CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Start,
    Stop,
    Restart,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    #[cfg(not(windows))]
    {
        let _ = cli;
        eprintln!("Hisoka service management is only supported on Windows.");
        return ExitCode::FAILURE;
    }

    #[cfg(windows)]
    {
        match cli.command {
            Commands::Start => service_command("start"),
            Commands::Stop => service_command("stop"),
            Commands::Restart => {
                match service_command("stop") {
                    ExitCode::SUCCESS => {}
                    code => return code,
                }

                // Give Windows time to finish stopping the service.
                thread::sleep(Duration::from_secs(1));

                service_command("start")
            }
        }
    }
}

#[cfg(windows)]
fn service_command(action: &str) -> ExitCode {
    let output = match Command::new("sc.exe")
        .args([action, SERVICE_NAME])
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            eprintln!("Failed to execute sc.exe: {error}");
            return ExitCode::FAILURE;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !stdout.trim().is_empty() {
        print!("{stdout}");
    }

    if !stderr.trim().is_empty() {
        eprint!("{stderr}");
    }

    let failed = !output.status.success()
        || stdout.contains("FAILED")
        || stderr.contains("FAILED");

    if failed {
        eprintln!("Failed to {action} the {SERVICE_NAME} service.");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}