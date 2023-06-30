use windows_service::{
    service::{ServiceAccess, ServiceState},
    service_manager::{ServiceManager, ServiceManagerAccess},
};

use std::os::windows::ffi::OsStrExt;
use std::ptr::null_mut;
use std::slice;
use std::{ffi::OsStr, mem};
use winapi::shared::{minwindef::FILETIME, ntdef::LPWSTR};
use winapi::um::wincred::{
    CredDeleteW, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
};
use winapi::{ctypes::c_ushort, shared::winerror::ERROR_NOT_FOUND};

use crate::daemon::AuthKind;

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

fn to_wide_chars(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

pub fn write_credentials(auth: AuthKind) -> anyhow::Result<()> {
    match auth {
        AuthKind::AuthPassword { username, password } => {
            let target = to_wide_chars("GEPH_DAEMON_AUTH");

            let combined_credential = format!("{}:{}", username, password);
            let combined_credential_wide = to_wide_chars(&combined_credential);

            // Create credential
            let mut credential = CREDENTIALW {
                Flags: 0,
                Type: CRED_TYPE_GENERIC,
                TargetName: target.as_ptr() as LPWSTR,
                Comment: null_mut(),
                LastWritten: FILETIME {
                    dwLowDateTime: 0,
                    dwHighDateTime: 0,
                },
                CredentialBlobSize: (combined_credential.len() * std::mem::size_of::<c_ushort>())
                    as u32,

                CredentialBlob: combined_credential_wide.as_ptr() as *mut _,
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                AttributeCount: 0,
                Attributes: null_mut(),
                TargetAlias: null_mut(),
                UserName: to_wide_chars(username.as_str()).as_ptr() as LPWSTR,
            };

            // Write credential
            let result = unsafe { CredWriteW(&mut credential, 0) };
            if result == 0 {
                anyhow::bail!("failed to save daemon auth");
            } else {
                println!("Credential saved successfully");
            }
        }
        _ => todo!(),
    }

    Ok(())
}
