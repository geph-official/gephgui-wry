#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(unix)]
pub fn configure_proxy() -> anyhow::Result<()> {
    use crate::daemon::PAC_ADDR;

    let mut cmd = std::process::Command::new("pac")
        .arg("on")
        .arg(format!("http://{}/proxy.pac", PAC_ADDR))
        .spawn()?;
    cmd.wait()?;
    Ok(())
}

#[cfg(unix)]
pub fn deconfigure_proxy() -> anyhow::Result<()> {
    let mut cmd = std::process::Command::new("pac").arg("off").spawn()?;
    cmd.wait()?;
    Ok(())
}

#[cfg(windows)]
pub fn configure_proxy() -> anyhow::Result<()> {
    use crate::daemon::HTTP_ADDR;

    let mut cmd = std::process::Command::new("winproxy-stripped")
        .arg("-proxy")
        .creation_flags(0x08000000)
        .arg(format!("http://{HTTP_ADDR}"))
        .spawn()?;
    cmd.wait()?;
    Ok(())
}

#[cfg(windows)]
pub fn deconfigure_proxy() -> anyhow::Result<()> {
    let mut cmd = std::process::Command::new("winproxy-stripped")
        .arg("-unproxy")
        .creation_flags(0x08000000)
        .spawn()?;
    cmd.wait()?;
    Ok(())
}
