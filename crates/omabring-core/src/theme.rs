use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// Colors read from an Omarchy theme `colors.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Palette {
    pub mode: String,
    pub dark: bool,
    pub background: String,
    pub foreground: String,
    pub accent: String,
    pub selection: String,
    pub muted: String,
    pub red: String,
    pub green: String,
    pub yellow: String,
    pub blue: String,
    pub darker_background: String,
    pub lighter_background: String,
}

impl Palette {
    pub fn fallback() -> Self {
        Self {
            mode: "dark".into(),
            dark: true,
            background: "#1e1e2e".into(),
            foreground: "#cdd6f4".into(),
            accent: "#89b4fa".into(),
            selection: "#45475a".into(),
            muted: "#6c7086".into(),
            red: "#f38ba8".into(),
            green: "#a6e3a1".into(),
            yellow: "#f9e2af".into(),
            blue: "#89b4fa".into(),
            darker_background: "#11111b".into(),
            lighter_background: "#313244".into(),
        }
    }

    pub fn to_css(&self) -> String {
        let accent_fg = contrast_fg(&self.accent);
        let destructive_fg = contrast_fg(&self.red);
        let success_fg = contrast_fg(&self.green);
        let warning_fg = contrast_fg(&self.yellow);
        format!(
            r#"/* OmaBring follows the active Omarchy theme. */
@define-color window_bg_color {bg};
@define-color window_fg_color {fg};
@define-color view_bg_color {bg};
@define-color view_fg_color {fg};
@define-color headerbar_bg_color {darker};
@define-color headerbar_fg_color {fg};
@define-color headerbar_border_color {selection};
@define-color sidebar_bg_color {darker};
@define-color sidebar_fg_color {fg};
@define-color sidebar_border_color {selection};
@define-color card_bg_color {lighter};
@define-color card_fg_color {fg};
@define-color popover_bg_color {lighter};
@define-color popover_fg_color {fg};
@define-color dialog_bg_color {lighter};
@define-color dialog_fg_color {fg};
@define-color accent_bg_color {accent};
@define-color accent_fg_color {accent_fg};
@define-color accent_color {accent};
@define-color destructive_bg_color {red};
@define-color destructive_fg_color {destructive_fg};
@define-color destructive_color {red};
@define-color success_bg_color {green};
@define-color success_color {green};
@define-color success_fg_color {success_fg};
@define-color warning_bg_color {yellow};
@define-color warning_color {yellow};
@define-color warning_fg_color {warning_fg};
@define-color error_bg_color {red};
@define-color error_color {red};
@define-color error_fg_color {destructive_fg};

window, .sidebar {{
  background-color: @window_bg_color;
  color: @window_fg_color;
}}
"#,
            bg = self.background,
            fg = self.foreground,
            darker = self.darker_background,
            lighter = self.lighter_background,
            selection = self.selection,
            accent = self.accent,
            accent_fg = accent_fg,
            red = self.red,
            green = self.green,
            yellow = self.yellow,
            destructive_fg = destructive_fg,
            success_fg = success_fg,
            warning_fg = warning_fg,
        )
    }
}

/// Resolve the active Omarchy theme directory.
///
/// Prefers an explicit override, then `~/.config/omarchy/current/theme`, then
/// `~/.local/state/omarchy/current/theme`.
pub fn resolve_theme_dir_at(home: &Path, override_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = override_dir {
        if path.as_os_str().is_empty() {
            return None;
        }
        if path.exists() {
            return Some(path.to_path_buf());
        }
    }
    let candidates = [
        home.join(".config/omarchy/current/theme"),
        home.join(".local/state/omarchy/current/theme"),
    ];
    for candidate in candidates {
        if candidate.join("colors.toml").is_file() || candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

pub fn resolve_theme_dir(home: &Path) -> Option<PathBuf> {
    let from_env = std::env::var("OMABRING_THEME")
        .ok()
        .filter(|v| !v.is_empty());
    resolve_theme_dir_at(home, from_env.as_deref().map(Path::new))
}

pub fn load_palette_str(text: &str) -> Option<Palette> {
    let value: toml::Value = toml::from_str(text).ok()?;
    let table = value.as_table()?;
    let pick = |keys: &[&str]| -> Option<String> {
        for key in keys {
            if let Some(toml::Value::String(color)) = table.get(*key) {
                if let Some(normalized) = normalize_hex(color) {
                    return Some(normalized);
                }
            }
        }
        None
    };
    let background = pick(&["background", "bg"])?;
    let foreground = pick(&["foreground", "fg"])?;
    let accent = pick(&["accent", "blue", "color4"]).unwrap_or_else(|| foreground.clone());
    let selection = pick(&["selection", "selection_background", "color0"])
        .unwrap_or_else(|| background.clone());
    let muted = pick(&["muted", "dark_foreground", "dark_fg", "color8"])
        .unwrap_or_else(|| foreground.clone());
    let red = pick(&["red", "color1"]).unwrap_or_else(|| "#f38ba8".into());
    let green = pick(&["green", "color2"]).unwrap_or_else(|| "#a6e3a1".into());
    let yellow = pick(&["yellow", "color3"]).unwrap_or_else(|| "#f9e2af".into());
    let blue = pick(&["blue", "color4"]).unwrap_or_else(|| accent.clone());
    let darker_background = pick(&[
        "darker_background",
        "darker_bg",
        "dark_background",
        "dark_bg",
    ])
    .unwrap_or_else(|| background.clone());
    let lighter_background =
        pick(&["lighter_background", "lighter_bg"]).unwrap_or_else(|| selection.clone());
    let mode = table
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or(if luminance(&background) < 0.5 {
            "dark"
        } else {
            "light"
        })
        .to_string();
    let dark = mode != "light" && luminance(&background) < 0.6;
    Some(Palette {
        mode,
        dark,
        background,
        foreground,
        accent,
        selection,
        muted,
        red,
        green,
        yellow,
        blue,
        darker_background,
        lighter_background,
    })
}

pub fn load_palette_path(path: &Path) -> Option<Palette> {
    let file = if path.is_dir() {
        path.join("colors.toml")
    } else {
        path.to_path_buf()
    };
    let text = fs::read_to_string(file).ok()?;
    load_palette_str(&text)
}

pub fn load_active_palette(home: &Path) -> Palette {
    resolve_theme_dir(home)
        .and_then(|dir| load_palette_path(&dir))
        .unwrap_or_else(Palette::fallback)
}

fn normalize_hex(raw: &str) -> Option<String> {
    let text = raw.trim();
    let hex = text.strip_prefix('#').unwrap_or(text);
    let expanded = match hex.len() {
        3 | 4 => hex.chars().take(3).flat_map(|c| [c, c]).collect::<String>(),
        6 | 8 => hex.chars().take(6).collect(),
        _ => return None,
    };
    if !expanded.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("#{}", expanded.to_ascii_lowercase()))
}

fn rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let normalized = normalize_hex(hex)?;
    let bytes = &normalized[1..];
    Some((
        u8::from_str_radix(&bytes[0..2], 16).ok()?,
        u8::from_str_radix(&bytes[2..4], 16).ok()?,
        u8::from_str_radix(&bytes[4..6], 16).ok()?,
    ))
}

fn channel(value: u8) -> f64 {
    let c = value as f64 / 255.0;
    if c <= 0.03928 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(hex: &str) -> f64 {
    let Some((r, g, b)) = rgb(hex) else {
        return 0.0;
    };
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

fn contrast_fg(bg: &str) -> String {
    if luminance(bg) > 0.45 {
        "#1a1a1a".into()
    } else {
        "#ffffff".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_palette() {
        let text = r##"
mode = "dark"
accent = "#89b4fa"
selection = "#45475a"
muted = "#585b70"
background = "#1e1e2e"
darker_background = "#101019"
lighter_background = "#313244"
foreground = "#cdd6f4"
red = "#f38ba8"
yellow = "#f9e2af"
green = "#a6e3a1"
blue = "#89b4fa"
"##;
        let palette = load_palette_str(text).unwrap();
        assert!(palette.dark);
        assert_eq!(palette.background, "#1e1e2e");
        assert_eq!(palette.accent, "#89b4fa");
        assert_eq!(palette.darker_background, "#101019");
        let css = palette.to_css();
        assert!(css.contains("@define-color accent_bg_color #89b4fa;"));
        assert!(css.contains("@define-color window_bg_color #1e1e2e;"));
    }

    #[test]
    fn parses_legacy_keys_and_light_mode() {
        let text = r##"
mode = "light"
bg = "#fafafa"
fg = "#1c1c1c"
color1 = "#c00000"
color2 = "#008000"
color3 = "#c0a000"
color4 = "#0044cc"
selection_background = "#d0d8e8"
"##;
        let palette = load_palette_str(text).unwrap();
        assert!(!palette.dark);
        assert_eq!(palette.background, "#fafafa");
        assert_eq!(palette.accent, "#0044cc");
        assert_eq!(palette.red, "#c00000");
        assert_eq!(palette.selection, "#d0d8e8");
    }

    #[test]
    fn resolves_config_theme_before_state_theme() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join(".config/omarchy/current/theme");
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("colors.toml"),
            "background = \"#111111\"\nforeground = \"#eeeeee\"\n",
        )
        .unwrap();
        let state = home.path().join(".local/state/omarchy/current/theme");
        fs::create_dir_all(&state).unwrap();
        fs::write(
            state.join("colors.toml"),
            "background = \"#ffffff\"\nforeground = \"#000000\"\n",
        )
        .unwrap();
        let dir = resolve_theme_dir_at(home.path(), None).unwrap();
        let palette = load_palette_path(&dir).unwrap();
        assert_eq!(palette.background, "#111111");
    }
}
