
use std::{
    ffi::OsString,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

const SERVICE_NAME: &str = "Hisoka";

#[cfg(windows)]
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode,
        ServiceState, ServiceStatus, ServiceType,
    },
    service_control_handler::{
        self, ServiceControlHandlerResult,
    },
    service_dispatcher,
};

#[cfg(windows)]
define_windows_service!(ffi_service_main, service_main);

#[cfg(windows)]
fn service_main(_arguments: Vec<OsString>) {
    if let Err(error) = run_service() {
        eprintln!("Hisoka service error: {error}");
    }
}

#[cfg(windows)]
fn run_service() -> Result<(), windows_service::Error> {
    let stopping = Arc::new(AtomicBool::new(false));
    let stopping_handler = Arc::clone(&stopping);

    let status_handle = service_control_handler::register(
        SERVICE_NAME,
        move |control| match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                stopping_handler.store(true, Ordering::SeqCst);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => {
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        },
    )?;

    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP
            | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    // Placeholder background workload.
    // Replace this loop with Hisoka's real service logic.
    while !stopping.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(500));
    }

    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::StopPending,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 1,
        wait_hint: Duration::from_secs(5),
        process_id: None,
    })?;

    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    Ok(())
}

#[cfg(windows)]
fn main() {
    if let Err(error) =
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
    {
        eprintln!("Could not start Hisoka service: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Hisoka service can only run on Windows.");
    std::process::exit(1);
}