#[cfg(unix)]
pub fn configure_proxy() -> anyhow::Result<()> {
    let mut cmd = std::process::Command::new("pac")
        .arg("on")
        .arg("http://127.0.0.1:9809/proxy.pac")
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
    let mut cmd = std::process::Command::new("winproxy-stripped")
        .arg("-proxy")
        .arg("http://127.0.0.1:9910")
        .spawn()?;
    cmd.wait()?;
    Ok(())
}

#[cfg(windows)]
pub fn deconfigure_proxy() -> anyhow::Result<()> {
    let mut cmd = std::process::Command::new("winproxy-stripped")
        .arg("-unproxy")
        .spawn()?;
    cmd.wait()?;
    Ok(())
}
