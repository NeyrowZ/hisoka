use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::{path::PathBuf, time::Duration};
use windows_service::{
    service::{ServiceAccess, ServiceState},
    service_manager::{ServiceManager, ServiceManagerAccess},
};

const SERVICE_NAME: &str = "Hisoka";

#[derive(Parser)]
#[command(name = "hisoka", version, about = "Manage the Hisoka background service")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show whether the service is running
    Status,
    /// Start the service (needs an administrator terminal)
    Start,
    /// Stop the service (needs an administrator terminal)
    Stop,
    /// Stop then start the service
    Restart,
    /// Print the last lines of the service log
    Logs {
        #[arg(short, long, default_value_t = 20)]
        lines: usize,
    },
}

fn app_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("cannot locate hisoka.exe")?;
    exe.parent()
        .and_then(|bin| bin.parent())
        .map(|p| p.to_path_buf())
        .context("unexpected install layout")
}

fn open(access: ServiceAccess) -> Result<windows_service::service::Service> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("cannot connect to the Service Control Manager")?;
    manager
        .open_service(SERVICE_NAME, access)
        .context("cannot open the Hisoka service (is it installed? are you admin?)")
}

fn state() -> Result<ServiceState> {
    Ok(open(ServiceAccess::QUERY_STATUS)?.query_status()?.current_state)
}

fn wait_for(target: ServiceState) -> Result<()> {
    for _ in 0..30 {
        if state()? == target {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    anyhow::bail!("timed out waiting for the service to reach {target:?}")
}

fn start() -> Result<()> {
    open(ServiceAccess::START)?.start::<&str>(&[])?;
    wait_for(ServiceState::Running)
}

fn stop() -> Result<()> {
    open(ServiceAccess::STOP)?.stop()?;
    wait_for(ServiceState::Stopped)
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Cmd::Status => println!("Hisoka: {:?}", state()?),
        Cmd::Start => { start()?; println!("started"); }
        Cmd::Stop => { stop()?; println!("stopped"); }
        Cmd::Restart => {
            if state()? != ServiceState::Stopped { stop()?; }
            start()?;
            println!("restarted");
        }
        Cmd::Logs { lines } => {
            let dir = app_dir()?.join("logs");
            let newest = std::fs::read_dir(&dir)
                .with_context(|| format!("cannot read {}", dir.display()))?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "log"))
                .max() // names embed the date, so the max name is the newest
                .context("no log files yet")?;
            let text = std::fs::read_to_string(&newest)?;
            let all: Vec<&str> = text.lines().collect();
            for l in &all[all.len().saturating_sub(lines)..] {
                println!("{l}");
            }
        }
    }
    Ok(())
}