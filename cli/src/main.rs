use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use hisoka_core::Config;
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
enum ConfigCmd {
    Show,
    Path,
    Set { key: String, value: String },
    Reset,
}

#[derive(Subcommand)]
enum WildcardCmd {
    List,
    Add {
        id: String,
        pattern: String,
        address: String,
    },
    Set {
        id: String,
        #[arg(long)]
        pattern: Option<String>,
        #[arg(long)]
        address: Option<String>,
    },
    Remove { id: String },
}

#[derive(Subcommand)]
enum Cmd {
    Status,
    Start,
    Stop,
    Restart,
    Logs {
        #[arg(short, long, default_value_t = 20)]
        lines: usize,
    },
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },
    Wildcard {
        #[command(subcommand)]
        action: WildcardCmd,
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
        let current = state()?;
        if std::mem::discriminant(&current) == std::mem::discriminant(&target) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(anyhow::anyhow!("timed out waiting for the service to reach {target:?}"))
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
        Cmd::Config { action } => {
            let path = app_dir()?.join("config.toml");
            match action {
                ConfigCmd::Path => println!("{}", path.display()),
                ConfigCmd::Show => print!("{}", Config::load(&path)?.to_toml_string()?),
                ConfigCmd::Set { key, value } => {
                    let mut cfg = Config::load(&path)?;
                    cfg.set_key(&key, &value)?;
                    cfg.save(&path)?;
                    println!("{key} = {value}  (run `hisoka restart` to apply)");
                }
                ConfigCmd::Reset => {
                    Config::default().save(&path)?;
                    println!("defaults restored  (run `hisoka restart` to apply)");
                }
            }
        }Cmd::Wildcard { action } => {
            let path = app_dir()?.join("config.toml");
            let mut cfg = Config::load(&path)?;
            match action {
                WildcardCmd::List => {
                    if cfg.wildcard.is_empty() {
                        println!("no wildcard rules (add one with `hisoka wildcard add`)");
                    }
                    for (id, r) in &cfg.wildcard {
                        println!("{id:<16} {:<30} -> {}", r.pattern, r.address);
                    }
                }
                WildcardCmd::Add { id, pattern, address } => {
                    cfg.add_rule(&id, &pattern, &address)?;
                    cfg.save(&path)?;
                    println!("added '{id}'  (run `hisoka restart` to apply)");
                }
                WildcardCmd::Set { id, pattern, address } => {
                    cfg.set_rule(&id, pattern.as_deref(), address.as_deref())?;
                    cfg.save(&path)?;
                    println!("updated '{id}'  (run `hisoka restart` to apply)");
                }
                WildcardCmd::Remove { id } => {
                    cfg.remove_rule(&id)?;
                    cfg.save(&path)?;
                    println!("removed '{id}'  (run `hisoka restart` to apply)");
                }
            }
        }
    }
    Ok(())
}