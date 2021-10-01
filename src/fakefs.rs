use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "gephgui/build/"]
pub struct FakeFs;
