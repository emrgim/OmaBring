use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
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

    /// Stylesheet for the active palette.
    ///
    /// Values are literal hex custom properties. Libadwaita derives
    /// `accent_color` with `oklab(from accent_bg_color)` and publishes the
    /// same names as CSS variables (`--accent-bg-color: @accent_bg_color`,
    /// and the same for window, destructive, success, warning, and error).
    /// Redefining those names with `@define-color` makes that resolution
    /// recurse until the main thread overflows. Hex variables do not point
    /// back at the named colors.
    pub fn to_css(&self) -> String {
        let accent_fg = contrast_fg(&self.accent);
        let destructive_fg = contrast_fg(&self.red);
        let success_fg = contrast_fg(&self.green);
        let warning_fg = contrast_fg(&self.yellow);
        format!(
            r#"/* OmaBring follows the active Omarchy theme. */
:root, window.omabring {{
  --window-bg-color: {bg};
  --window-fg-color: {fg};
  --view-bg-color: {bg};
  --view-fg-color: {fg};
  --headerbar-bg-color: {darker};
  --headerbar-fg-color: {fg};
  --headerbar-border-color: {selection};
  --sidebar-bg-color: {darker};
  --sidebar-fg-color: {fg};
  --sidebar-border-color: {selection};
  --card-bg-color: {lighter};
  --card-fg-color: {fg};
  --popover-bg-color: {lighter};
  --popover-fg-color: {fg};
  --dialog-bg-color: {lighter};
  --dialog-fg-color: {fg};
  --accent-bg-color: {accent};
  --accent-fg-color: {accent_fg};
  --accent-color: {accent};
  --destructive-bg-color: {red};
  --destructive-fg-color: {destructive_fg};
  --destructive-color: {red};
  --success-bg-color: {green};
  --success-fg-color: {success_fg};
  --success-color: {green};
  --warning-bg-color: {yellow};
  --warning-fg-color: {warning_fg};
  --warning-color: {yellow};
  --error-bg-color: {red};
  --error-fg-color: {destructive_fg};
  --error-color: {red};
}}

window.omabring,
window.omabring .sidebar {{
  background-color: {bg};
  color: {fg};
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
        if theme_colors_file(&candidate).is_some() {
            return Some(candidate);
        }
    }
    None
}

/// Resolve `path` to a readable `colors.toml`, refusing symlink cycles.
///
/// A cycle between `~/.config/omarchy/current/theme` and
/// `~/.local/state/omarchy/current/theme` is not a theme. The walk is
/// iterative and bounded, so a loop cannot grow the stack.
fn theme_colors_file(path: &Path) -> Option<PathBuf> {
    let resolved = follow_limited(path, 16)?;
    let file = if resolved.is_dir() {
        follow_limited(&resolved.join("colors.toml"), 16)?
    } else {
        resolved
    };
    if file.is_file() {
        Some(file)
    } else {
        None
    }
}

fn follow_limited(path: &Path, max_hops: usize) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    let mut seen = HashSet::new();
    for _ in 0..=max_hops {
        let meta = fs::symlink_metadata(&current).ok()?;
        if !seen.insert((meta.dev(), meta.ino())) {
            return None;
        }
        if !meta.file_type().is_symlink() {
            return Some(current);
        }
        let target = fs::read_link(&current).ok()?;
        current = if target.is_absolute() {
            target
        } else {
            current
                .parent()
                .unwrap_or_else(|| Path::new("/"))
                .join(target)
        };
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
    let file = theme_colors_file(path)?;
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
        assert!(!css.contains("@define-color"));
        assert!(!css.contains("@accent_"));
        assert!(!css.contains("@window_"));
        assert!(css.contains("--accent-bg-color: #89b4fa;"));
        assert!(css.contains("--accent-color: #89b4fa;"));
        assert!(css.contains("--window-bg-color: #1e1e2e;"));
        assert!(css.contains("background-color: #1e1e2e;"));
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

    #[test]
    fn follows_config_symlink_to_state_theme() {
        let home = tempfile::tempdir().unwrap();
        let state = home.path().join(".local/state/omarchy/current/theme");
        fs::create_dir_all(&state).unwrap();
        fs::write(
            state.join("colors.toml"),
            "background = \"#222222\"\nforeground = \"#eeeeee\"\n",
        )
        .unwrap();
        let config_parent = home.path().join(".config/omarchy/current");
        fs::create_dir_all(&config_parent).unwrap();
        std::os::unix::fs::symlink(&state, config_parent.join("theme")).unwrap();
        let palette = load_active_palette(home.path());
        assert_eq!(palette.background, "#222222");
    }

    #[test]
    fn symlink_cycle_falls_back_without_recursing() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join(".config/omarchy/current/theme");
        let state = home.path().join(".local/state/omarchy/current/theme");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::create_dir_all(state.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&state, &config).unwrap();
        std::os::unix::fs::symlink(&config, &state).unwrap();
        assert!(resolve_theme_dir_at(home.path(), None).is_none());
        let palette = load_active_palette(home.path());
        assert_eq!(palette, Palette::fallback());
        assert!(load_palette_path(&config).is_none());
        assert!(load_palette_path(&state).is_none());
    }
}
