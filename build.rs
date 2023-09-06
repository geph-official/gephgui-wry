use std::io;
#[cfg(windows)]
use winres::WindowsResource;

fn main() -> io::Result<()> {
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
