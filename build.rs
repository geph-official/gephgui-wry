use std::io;
#[cfg(windows)]
use winres::WindowsResource;

fn main() -> io::Result<()> {
    // The version shown in-app and used by the auto-updater is read via
    // option_env!("VERSION") at compile time. Track it so bumping VERSION forces
    // a rebuild instead of baking in a stale (or missing) value from the cache.
    println!("cargo:rerun-if-env-changed=VERSION");

    #[cfg(windows)]
    {
        // This embeds a Windows manifest into the Rust executable to prompt the user for administrator privileges.
        // embed_resource::compile("gephgui-wry-manifest.rc", embed_resource::NONE);

        WindowsResource::new()
            // This path can be absolute, or relative to your crate root.
            .set_icon("src/logo-naked.ico")
            .compile()?;
    }

    Ok(())
}
