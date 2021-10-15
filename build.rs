use std::io;
#[cfg(windows)] use winres::WindowsResource;

fn main() -> io::Result<()> {
    #[cfg(windows)] {
        WindowsResource::new()
            // This path can be absolute, or relative to your crate root.
            .set_icon("src/logo-naked.ico")
            .compile()?;
    }
    Ok(())
}
