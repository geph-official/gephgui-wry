use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "gephgui/dist/"]
pub struct FakeFs;
