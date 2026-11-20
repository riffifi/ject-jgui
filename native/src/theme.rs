//! Themes stay a public JGUI choice; renderer/toolkit details stay private.
pub const NAMES: [&str; 6] = [
    "linen",
    "midnight",
    "metro-light",
    "metro-dark",
    "egui-light",
    "egui-dark",
];
pub fn validate(name: &str) -> Result<(), String> {
    if !NAMES.contains(&name) {
        return Err(format!(
            "unknown theme '{name}'; choose {}",
            NAMES.join(", ")
        ));
    }
    Ok(())
}
pub fn named(name: &str) -> brockmann::Theme {
    use brockmann::{theme::Color, Accent, Theme};
    let light = matches!(name, "linen" | "metro-light" | "egui-light");
    let mut theme = Theme::new(light, Accent::Teal);
    if name == "linen" {
        theme.bg = Color::hex(0xf5f3ed);
        theme.surface = Color::hex(0xfffefa);
        theme.fg = Color::hex(0x26312f);
        theme.accent = Color::hex(0x187466);
    } else if name == "midnight" {
        theme.bg = Color::hex(0x131924);
        theme.surface = Color::hex(0x1f2938);
        theme.fg = Color::hex(0xe3eaf3);
        theme.accent = Color::hex(0x64c3b9);
    }
    theme
}
