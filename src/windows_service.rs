use std::ffi::OsString;

use winapi::shared::winerror::ERROR_SERVICE_DOES_NOT_EXIST;
use windows_service::{
    service::{
        ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState,
        ServiceType,
    },
    service_manager::{ServiceManager, ServiceManagerAccess},
    Error,
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

pub fn is_service_installed(service_name: &str) -> anyhow::Result<bool> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;

    match manager.open_service(service_name, ServiceAccess::QUERY_STATUS) {
        Ok(_) => Ok(true),
        Err(err) => {
            match err {
                Error::Winapi(code) => {
                    if code.raw_os_error().unwrap() as u32 == ERROR_SERVICE_DOES_NOT_EXIST {
                        eprintln!("Service does not exist");
                        return Ok(false);
                    } else {
                        eprintln!("Error code: {}", code);
                    }
                }
                _ => anyhow::bail!("unknown winapi error occured: {}", err),
            }
            Ok(false)
        }
    }
}

pub fn install_windows_service() -> anyhow::Result<()> {
    if is_service_installed(SERVICE_NAME)? {
        eprintln!("{} service is already installed", SERVICE_NAME);
        return Ok(());
    }

    eprintln!("Installing Geph Daemon as a Windows service");

    let manager_access = ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE;
    let service_manager = ServiceManager::local_computer(None::<&str>, manager_access)?;

    let mut service_binary_path = dirs::home_dir().unwrap();
    service_binary_path.push(".cargo");
    service_binary_path.push("bin");
    service_binary_path.push("geph_daemon.exe");

    let service_info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from("Geph Daemon"),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: service_binary_path,
        launch_arguments: vec![],
        dependencies: vec![],
        account_name: None, // run as System
        account_password: None,
    };
    let service = service_manager.create_service(&service_info, ServiceAccess::CHANGE_CONFIG)?;
    service.set_description("Geph Daemon Windows Service (geph4-client)")?;
    eprintln!("Successfully installed Windows service");

    Ok(())
}
