use windows_service::{
    service::{ServiceAccess, ServiceState},
    service_manager::{ServiceManager, ServiceManagerAccess},
};

const SERVICE_NAME: &str = "GephDaemon";

pub fn is_service_running() -> anyhow::Result<bool> {
    let manager_access = ServiceManagerAccess::CONNECT;
    let service_manager = ServiceManager::local_computer(None::<&str>, manager_access)?;

    let service = service_manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS)?;

    let service_status = service.query_status()?;

    Ok(service_status.current_state == ServiceState::Running)
}

pub fn start_service() -> anyhow::Result<()> {
    let manager_access = ServiceManagerAccess::CONNECT | ServiceManagerAccess::CONNECT;
    let service_manager = ServiceManager::local_computer(None::<&str>, manager_access)?;
    let service = service_manager.open_service(SERVICE_NAME, ServiceAccess::START)?;

    eprintln!("Starting Geph Daemon Windows service...");
    let args: Vec<&str> = Vec::new();
    service.start(args.as_slice())?;
    eprintln!("Successfully started Geph Daemon Windows service!");
    Ok(())
}

pub fn stop_service() -> anyhow::Result<()> {
    let manager_access = ServiceManagerAccess::CONNECT;
    let service_manager = ServiceManager::local_computer(None::<&str>, manager_access)?;

    let service = service_manager.open_service(
        SERVICE_NAME,
        ServiceAccess::QUERY_STATUS | ServiceAccess::STOP,
    )?;

    let mut retries = 5;
    loop {
        match service.query_status() {
            Ok(service_status) => {
                if service_status.current_state != ServiceState::StopPending
                    && service_status.current_state != ServiceState::Stopped
                {
                    eprintln!("Attempting to stop Geph Daemon Windows service...");
                    let result = service.stop();
                    match result {
                        Ok(_) => {
                            eprintln!("Successfully stopped Geph Daemon Windows service!");
                            break;
                        }
                        Err(_) => {
                            if retries == 0 {
                                return Err(anyhow::anyhow!(
                                    "Failed to stop the service after several attempts."
                                ));
                            }
                            retries -= 1;
                            std::thread::sleep(std::time::Duration::from_secs(2));
                        }
                    }
                } else if service_status.current_state == ServiceState::Stopped {
                    eprintln!("Geph Daemon Windows service is already stopped!");
                    break;
                } else {
                    if retries == 0 {
                        return Err(anyhow::anyhow!(
                            "Failed to stop the service after several attempts."
                        ));
                    }
                    retries -= 1;
                    std::thread::sleep(std::time::Duration::from_secs(2));
                }
            }
            Err(e) => return Err(e.into()),
        }
    }

    Ok(())
}
