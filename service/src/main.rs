use anyhow::{Context, Result as AnyResult};
use chrono::Local;
use hisoka_core::{dns, Config, LogConfig, Matcher, DNS_PORT};
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
    },
    time::{Duration, Instant, SystemTime},
    net::UdpSocket,
    sync::Arc,
    thread,
};
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult, ServiceStatusHandle},
    service_dispatcher,
};

const SERVICE_NAME: &str = "Hisoka";

/// When true, log lines are also printed to the terminal.
static CONSOLE: AtomicBool = AtomicBool::new(false);

define_windows_service!(ffi_service_main, service_main);

// ---------- paths + logging ----------

fn app_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("HISOKA_HOME") {
        return PathBuf::from(home);
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn config_path() -> PathBuf {
    app_dir().join("config.toml")
}

fn log_dir() -> PathBuf {
    app_dir().join("logs")
}

fn log(msg: &str) {
    let now = Local::now();
    let line = format!("[{}] {msg}", now.format("%H:%M:%S"));
    if CONSOLE.load(Ordering::Relaxed) {
        println!("{line}");
    }
    let dir = log_dir();
    let _ = fs::create_dir_all(&dir);
    let file = dir.join(format!("hisoka-{}.log", now.format("%Y-%m-%d")));
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(file) {
        let _ = writeln!(f, "{line}");
    }
}

fn prune_logs(cfg: &LogConfig) {
    let Ok(entries) = fs::read_dir(log_dir()) else { return };
    let cutoff = SystemTime::now()
        - Duration::from_secs(cfg.retention_days as u64 * 24 * 3600);
    let max_bytes = cfg.max_mb as u64 * 1024 * 1024;

    let mut files: Vec<(PathBuf, SystemTime, u64)> = entries
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.starts_with("hisoka-") && n.ends_with(".log")
        })
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((e.path(), m.modified().ok()?, m.len()))
        })
        .collect();

    files.retain(|(p, modified, _)| {
        if *modified < cutoff {
            let _ = fs::remove_file(p);
            false
        } else {
            true
        }
    });

    files.sort_by_key(|(_, modified, _)| *modified);
    let mut total: u64 = files.iter().map(|(_, _, s)| s).sum();
    for (p, _, size) in &files {
        if total <= max_bytes {
            break;
        }
        let _ = fs::remove_file(p);
        total -= size;
    }
}

// ---------- the shared logic ----------

fn serve(socket: UdpSocket, matcher: Matcher, debug: bool, stop: Arc<AtomicBool>) {
    let mut buf = [0u8; 1500];
    while !stop.load(Ordering::Relaxed) {
        let (n, peer) = match socket.recv_from(&mut buf) {
            Ok(x) => x,
            // Read timeout (lets us notice `stop`), or Windows' spurious
            // "connection reset" after a client vanished. Keep going.
            Err(_) => continue,
        };
        if let Some(reply) = dns::handle(&buf[..n], &matcher) {
            if debug {
                log(&format!("query: {}", reply.summary));
            }
            let _ = socket.send_to(&reply.packet, peer);
        }
    }
}

/// Everything the service does. Runs until something is sent on `stop`.
/// Both entry points below call this and nothing else.
fn worker(stop: Receiver<()>) -> AnyResult<()> {
    let cfg = match Config::load_or_create(&config_path()) {
        Ok(c) => c,
        Err(e) => {
            log(&format!("config error, using defaults: {e:#}"));
            Config::default()
        }
    };

    prune_logs(&cfg.log);
    let mut last_prune = Instant::now();

    log("Hisoka Service started.");
    log(&format!("config: {cfg:?}"));

    let socket = UdpSocket::bind(("127.0.0.1", DNS_PORT)).with_context(|| {
        format!("cannot listen on 127.0.0.1:{DNS_PORT}")
    })?;
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;

    let stop_flag = Arc::new(AtomicBool::new(false));
    let resolver = {
        let flag = stop_flag.clone();
        let matcher = cfg.matcher();
        let debug = cfg.log.debug;
        thread::spawn(move || serve(socket, matcher, debug, flag))
    };

    log(&format!("Resolver listening on 127.0.0.1:{DNS_PORT} with {} rule(s)", cfg.wildcard.len()));

    loop {
        match stop.recv_timeout(Duration::from_secs(5)) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                if last_prune.elapsed() >= Duration::from_secs(24 * 3600) {
                    prune_logs(&cfg.log);
                    last_prune = Instant::now();
                }
            }
        }
    }

    stop_flag.store(true, Ordering::Relaxed);
    let _ = resolver.join();
    log("Hisoka Service stopping.");
    Ok(())
}

// ---------- entry point 1: console ----------

fn console_main() {
    CONSOLE.store(true, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel::<()>();
    if let Err(e) = ctrlc::set_handler(move || {
        let _ = tx.send(());
    }) {
        eprintln!("cannot install Ctrl+C handler: {e}");
        return;
    }
    println!("running in console mode, Ctrl+C to stop");
    if let Err(e) = worker(rx) {
        log(&format!("service error: {e:#}"));
    }
}

// ---------- entry point 2: Windows service ----------

fn set_state(
    handle: &ServiceStatusHandle,
    state: ServiceState,
    exit_code: u32,
) -> windows_service::Result<()> {
    handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: ServiceExitCode::Win32(exit_code),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })
}

fn service_main(_args: Vec<OsString>) {
    let (tx, rx) = mpsc::channel::<()>();

    let handle = match service_control_handler::register(SERVICE_NAME, move |event| match event {
        ServiceControl::Stop => {
            let _ = tx.send(());
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    }) {
        Ok(h) => h,
        Err(e) => {
            log(&format!("cannot register with the SCM: {e}"));
            return;
        }
    };

    let _ = set_state(&handle, ServiceState::Running, 0);

    let code = match worker(rx) {
        Ok(()) => 0,
        Err(e) => {
            log(&format!("service error: {e:#}"));
            1
        }
    };

    let _ = set_state(&handle, ServiceState::Stopped, code);
}

// ---------- main ----------

fn main() -> Result<(), windows_service::Error> {
    if std::env::args().any(|a| a == "--console") {
        console_main();
        return Ok(());
    }
    // Fails with error 1063 if started from a terminal without --console.
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
}