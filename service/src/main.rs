use chrono::Local;
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::mpsc::{self, RecvTimeoutError},
    time::{Instant, SystemTime, Duration},
};
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};

const SERVICE_NAME: &str = "Hisoka";

define_windows_service!(ffi_service_main, service_main);

fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

const RETENTION_DAYS: u64 = 7;          // later: read from config.toml
const MAX_TOTAL_BYTES: u64 = 10 * 1024 * 1024;

fn log_dir() -> PathBuf {
    app_dir().join("logs")
}

fn log(msg: &str) {
    let now = Local::now();
    let dir = log_dir();
    let _ = fs::create_dir_all(&dir);
    let file = dir.join(format!("hisoka-{}.log", now.format("%Y-%m-%d")));
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(file) {
        let _ = writeln!(f, "{} {msg}", now.format("%H:%M:%S"));
    }
}

fn prune_logs() {
    let Ok(entries) = fs::read_dir(log_dir()) else { return };
    let cutoff = SystemTime::now() - Duration::from_secs(RETENTION_DAYS * 24 * 3600);

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
        if total <= MAX_TOTAL_BYTES {
            break;
        }
        let _ = fs::remove_file(p);
        total -= size;
    }
}

fn main() -> Result<(), windows_service::Error> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
}

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run() {
        log(&format!("service error: {e}"));
    }
}

fn set_state(
    handle: &service_control_handler::ServiceStatusHandle,
    state: ServiceState,
) -> windows_service::Result<()> {
    handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })
}

fn run() -> windows_service::Result<()> {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();

    let handle = service_control_handler::register(SERVICE_NAME, move |event| match event {
        ServiceControl::Stop => {
            let _ = stop_tx.send(());
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;

    prune_logs();
    let mut last_prune = Instant::now();

    set_state(&handle, ServiceState::Running)?;
    log("service started");

    loop {
        match stop_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                if last_prune.elapsed() >= Duration::from_secs(24 * 3600) {
                    prune_logs();
                    last_prune = Instant::now();
                }
                // TODO ...
            }
        }
    }

    log("service stopping");
    set_state(&handle, ServiceState::Stopped)?;
    Ok(())
}