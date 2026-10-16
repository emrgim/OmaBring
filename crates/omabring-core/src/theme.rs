use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::Serialize;

const DEFAULT_FONT: &str = "JetBrainsMono Nerd Font";

/// Colors and the monospace face taken from the active Omarchy theme.
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
    pub border: String,
    pub font: String,
}

#[derive(Default)]
struct RawColors {
    mode: Option<String>,
    background: Option<String>,
    foreground: Option<String>,
    accent: Option<String>,
    selection: Option<String>,
    muted: Option<String>,
    red: Option<String>,
    green: Option<String>,
    yellow: Option<String>,
    blue: Option<String>,
    darker_background: Option<String>,
    lighter_background: Option<String>,
    dark_background: Option<String>,
    border: Option<String>,
}

impl RawColors {
    fn overlay(&mut self, other: Self) {
        macro_rules! take {
            ($($field:ident),* $(,)?) => {
                $(
                    if other.$field.is_some() {
                        self.$field = other.$field;
                    }
                )*
            };
        }
        take!(
            mode,
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
            dark_background,
            border,
        );
    }

    fn finish(self) -> Option<Palette> {
        let background = self.background?;
        let foreground = self.foreground?;
        let accent = self
            .accent
            .or_else(|| self.blue.clone())
            .unwrap_or_else(|| foreground.clone());
        let selection = self
            .selection
            .or_else(|| self.lighter_background.clone())
            .unwrap_or_else(|| background.clone());
        let muted = self.muted.unwrap_or_else(|| foreground.clone());
        let red = self.red.unwrap_or_else(|| "#f38ba8".into());
        let green = self.green.unwrap_or_else(|| "#a6e3a1".into());
        let yellow = self.yellow.unwrap_or_else(|| "#f9e2af".into());
        let blue = self.blue.unwrap_or_else(|| accent.clone());
        let darker_background = self
            .darker_background
            .or(self.dark_background)
            .unwrap_or_else(|| background.clone());
        let lighter_background = self.lighter_background.unwrap_or_else(|| selection.clone());
        let border = self.border.unwrap_or_else(|| with_alpha(&foreground, 0.40));
        let mode = self.mode.unwrap_or_else(|| {
            if channel_sum(&background) > 382 {
                "light".into()
            } else {
                "dark".into()
            }
        });
        let dark = match mode.as_str() {
            "light" => false,
            "dark" => true,
            _ => channel_sum(&background) <= 382,
        };
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
            border,
            font: DEFAULT_FONT.into(),
        })
    }
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
            border: with_alpha("#cdd6f4", 0.40),
            font: DEFAULT_FONT.into(),
        }
    }

    /// Stylesheet for the active palette.
    ///
    /// Colors live only under app-specific names (`--omabring-bg` and the
    /// rest) and are referenced from OmaBring selectors. Libadwaita names
    /// (`--window-bg-color`, `--accent-color`, and the other surface and
    /// accent variables) are not set, and there is no `@define-color` and no
    /// `:root` block. On GTK 4.22 with libadwaita 1.9, overriding those names
    /// makes style resolution recurse inside `gtk_widget_show` until the main
    /// thread overflows.
    pub fn to_css(&self) -> String {
        let accent_fg = contrast_fg(&self.accent);
        let font = css_font_family(&self.font);
        format!(
            r#"/* OmaBring follows the active Omarchy theme. */
window.omabring {{
  --omabring-bg: {bg};
  --omabring-surface: {lighter};
  --omabring-fg: {fg};
  --omabring-accent: {accent};
  --omabring-accent-fg: {accent_fg};
  --omabring-muted: {muted};
  --omabring-sidebar-bg: {darker};
  --omabring-card-bg: {lighter};
  --omabring-selection: {selection};
  --omabring-border: {border};
  --omabring-danger: {red};
  --omabring-ok: {green};
  --omabring-warn: {yellow};
  background-color: var(--omabring-bg);
  color: var(--omabring-fg);
  font-family: {font};
  font-size: 14px;
  border-radius: 0;
}}

window.omabring,
window.omabring * {{
  font-family: {font};
  border-radius: 0;
  box-shadow: none;
  outline: none;
}}

window.omabring label,
window.omabring entry,
window.omabring text,
window.omabring textview {{
  color: var(--omabring-fg);
}}

window.omabring .bf-header {{
  background-color: var(--omabring-bg);
  border-bottom: 1px dashed var(--omabring-border);
}}

window.omabring .bf-mark {{
  background-color: var(--omabring-accent);
  color: var(--omabring-accent-fg);
  font-weight: 700;
  font-size: 13px;
  min-width: 34px;
  min-height: 34px;
}}

window.omabring .bf-brand {{
  font-size: 16px;
  font-weight: 700;
  letter-spacing: 0.3px;
}}

window.omabring .bf-kicker,
window.omabring .bf-field,
window.omabring .bf-muted {{
  color: var(--omabring-muted);
  font-size: 11px;
  letter-spacing: 1px;
}}

window.omabring .bf-h1 {{
  font-size: 28px;
  font-weight: 700;
  letter-spacing: 0.4px;
}}

window.omabring .bf-h1-sm {{
  font-size: 20px;
  font-weight: 700;
  letter-spacing: 0.3px;
}}

window.omabring .bf-go {{
  color: var(--omabring-accent);
  font-size: 12px;
  font-weight: 700;
  letter-spacing: 0.8px;
}}

window.omabring .bf-delta {{
  font-size: 24px;
  font-weight: 700;
}}

window.omabring .bf-ok {{
  color: var(--omabring-ok);
  font-size: 12px;
  font-weight: 700;
}}

window.omabring .bf-danger {{
  color: var(--omabring-danger);
}}

window.omabring button {{
  background-color: var(--omabring-accent);
  color: var(--omabring-accent-fg);
  border: 1px solid var(--omabring-accent);
  border-radius: 0;
  padding: 8px 12px;
  min-height: 0;
  font-weight: 700;
  box-shadow: none;
}}

window.omabring button label {{
  color: var(--omabring-accent-fg);
  font-weight: 700;
}}

window.omabring button.bf-ghost,
window.omabring button.bf-ghost label {{
  background-color: transparent;
  color: var(--omabring-fg);
  border-color: var(--omabring-border);
}}

window.omabring button.bf-chip,
window.omabring button.bf-tab,
window.omabring button.bf-chip label,
window.omabring button.bf-tab label {{
  background-color: transparent;
  color: var(--omabring-muted);
  border: 1px solid var(--omabring-border);
  padding: 4px 8px;
  font-size: 11px;
  letter-spacing: 0.4px;
  min-height: 0;
}}

window.omabring button.bf-chip.on,
window.omabring button.bf-tab.on,
window.omabring button.bf-chip.on label,
window.omabring button.bf-tab.on label {{
  background-color: var(--omabring-accent);
  color: var(--omabring-accent-fg);
  border-color: var(--omabring-accent);
}}

window.omabring .bf-card {{
  background-color: var(--omabring-surface);
  color: var(--omabring-fg);
  border: 1px solid var(--omabring-border);
  padding: 16px;
}}

window.omabring .bf-dashed {{
  border-style: dashed;
}}

window.omabring .bf-pill {{
  color: var(--omabring-muted);
  border: 1px solid var(--omabring-border);
  padding: 2px 6px;
  font-size: 11px;
  font-weight: 700;
}}

window.omabring .bf-pill.on {{
  background-color: var(--omabring-accent);
  color: var(--omabring-accent-fg);
  border-color: var(--omabring-accent);
}}

window.omabring .bf-cap {{
  background-color: var(--omabring-accent);
  color: var(--omabring-accent-fg);
  padding: 2px 6px;
  font-size: 11px;
  font-weight: 700;
}}

window.omabring .bf-cap.off {{
  background-color: transparent;
  color: var(--omabring-muted);
  border: 1px dashed var(--omabring-border);
}}

window.omabring .bf-thumb,
window.omabring .bf-logo {{
  font-size: 18px;
  font-weight: 700;
  background-color: var(--omabring-selection);
  color: var(--omabring-fg);
  min-width: 48px;
  min-height: 48px;
}}

window.omabring .bf-logo {{
  min-width: 64px;
  min-height: 64px;
  font-size: 22px;
}}

window.omabring .logo-carrefour {{ background-color: #004e9f; color: #ffffff; }}
window.omabring .logo-union {{ background-color: #0b7a3e; color: #ffffff; }}
window.omabring .logo-grandiose {{ background-color: #1a1a1a; color: #d4af37; }}
window.omabring .logo-waitrose {{ background-color: #007a33; color: #ffffff; }}
window.omabring .logo-spinneys {{ background-color: #8b1e3f; color: #ffffff; }}
window.omabring .logo-mmi {{ background-color: #1a1a1a; color: #ffffff; }}
window.omabring .logo-african {{ background-color: #b11226; color: #ffffff; }}

window.omabring .bf-bar {{
  background-color: transparent;
  min-height: 110px;
}}

window.omabring .bf-bar-fill {{
  background-color: var(--omabring-fg);
  min-width: 8px;
}}

window.omabring .bf-bar.on {{
  border: 1px solid var(--omabring-accent);
}}

window.omabring .bf-row {{
  background-color: transparent;
  border-bottom: 1px dashed var(--omabring-border);
  padding: 8px 4px;
}}

window.omabring .bf-row:hover,
window.omabring .bf-row:hover label {{
  background-color: var(--omabring-accent);
  color: var(--omabring-accent-fg);
}}

window.omabring .bf-thead {{
  border-bottom: 1px solid var(--omabring-border);
  padding: 6px 4px;
}}

window.omabring .bf-thead label {{
  color: var(--omabring-muted);
  font-size: 11px;
  font-weight: 700;
}}

window.omabring button.bf-th,
window.omabring button.bf-th label {{
  background-color: transparent;
  color: var(--omabring-muted);
  border: none;
  padding: 0;
  font-size: 11px;
  font-weight: 700;
  min-height: 0;
}}

window.omabring button.bf-th.on,
window.omabring button.bf-th.on label {{
  background-color: transparent;
  color: var(--omabring-fg);
  border: none;
}}

window.omabring entry,
window.omabring passwordentry,
window.omabring searchentry,
window.omabring spinbutton {{
  background-color: var(--omabring-bg);
  color: var(--omabring-fg);
  border: 1px solid var(--omabring-border);
  border-radius: 0;
  box-shadow: none;
  min-height: 0;
  padding: 8px 10px;
}}

window.omabring textview,
window.omabring textview text {{
  background-color: var(--omabring-bg);
  color: var(--omabring-fg);
}}

window.omabring scrolledwindow,
window.omabring viewport {{
  background-color: var(--omabring-bg);
  border: none;
}}

window.omabring selection {{
  background-color: var(--omabring-accent);
  color: var(--omabring-accent-fg);
}}

window.omabring popover,
window.omabring popover contents {{
  background-color: var(--omabring-surface);
  color: var(--omabring-fg);
  border: 1px solid var(--omabring-border);
  border-radius: 0;
}}
"#,
            bg = self.background,
            fg = self.foreground,
            darker = self.darker_background,
            lighter = self.lighter_background,
            selection = self.selection,
            accent = self.accent,
            accent_fg = accent_fg,
            muted = self.muted,
            border = self.border,
            red = self.red,
            green = self.green,
            yellow = self.yellow,
            font = font,
        )
    }
}

/// Directories and files whose replacement should restyle the window.
///
/// `omarchy-theme-set` deletes `current/theme` and moves a new directory into
/// its place, so the watch is the parent `current` directory as well as the
/// theme directory itself. The fontconfig directory is included because the
/// monospace face lives there.
pub fn theme_watch_paths(home: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut push = |path: PathBuf| {
        if path.exists() && !paths.iter().any(|have| have == &path) {
            paths.push(path);
        }
    };
    push(home.join(".config/omarchy/current"));
    push(home.join(".local/state/omarchy/current"));
    push(home.join(".config/fontconfig"));
    if let Some(theme) = resolve_theme_dir(home) {
        if let Some(parent) = theme.parent() {
            push(parent.to_path_buf());
        }
        push(theme.clone());
        if let Some(resolved) = follow_limited(&theme, 16) {
            if resolved != theme {
                if let Some(parent) = resolved.parent() {
                    push(parent.to_path_buf());
                }
                push(resolved);
            }
        }
    }
    paths.sort();
    paths
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
        if theme_has_source(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn theme_has_source(path: &Path) -> bool {
    let Some(resolved) = follow_limited(path, 16) else {
        return false;
    };
    if resolved.is_file() {
        return is_color_file(&resolved);
    }
    resolved.is_dir() && dir_has_source(&resolved)
}

fn dir_has_source(dir: &Path) -> bool {
    const NAMES: &[&str] = &[
        "colors.toml",
        "alacritty.toml",
        "walker.css",
        "waybar.css",
        "style.css",
        "waybar/style.css",
        "walker/style.css",
    ];
    NAMES.iter().any(|name| dir.join(name).is_file()) || dir.join("light.mode").is_file()
}

fn is_color_file(path: &Path) -> bool {
    match path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
    {
        "colors.toml" | "alacritty.toml" | "walker.css" | "waybar.css" | "style.css" => true,
        _ => false,
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
    raw_from_colors_toml(text).finish()
}

pub fn load_palette_path(path: &Path) -> Option<Palette> {
    let resolved = follow_limited(path, 16)?;
    if resolved.is_file() {
        return raw_from_file(&resolved)?.finish();
    }
    if resolved.is_dir() {
        return load_palette_dir(&resolved);
    }
    None
}

fn load_palette_dir(dir: &Path) -> Option<Palette> {
    let mut raw = RawColors::default();
    // Later files win. colors.toml is the canonical palette; the terminal,
    // launcher, and bar styles fill gaps when a theme omits it.
    for name in [
        "waybar/style.css",
        "waybar.css",
        "style.css",
        "walker/style.css",
        "walker.css",
        "alacritty.toml",
        "colors.toml",
    ] {
        let file = dir.join(name);
        if file.is_file() {
            if let Some(parsed) = raw_from_file(&file) {
                raw.overlay(parsed);
            }
        }
    }
    if raw.mode.is_none() && dir.join("light.mode").is_file() {
        raw.mode = Some("light".into());
    }
    raw.finish()
}

fn raw_from_file(path: &Path) -> Option<RawColors> {
    let text = fs::read_to_string(path).ok()?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if name == "alacritty.toml" {
        return raw_from_alacritty(&text);
    }
    if name.ends_with(".css") {
        return raw_from_css(&text);
    }
    Some(raw_from_colors_toml(&text))
}

fn raw_from_colors_toml(text: &str) -> RawColors {
    let Ok(value) = toml::from_str::<toml::Value>(text) else {
        return RawColors::default();
    };
    let Some(table) = value.as_table() else {
        return RawColors::default();
    };
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
    let mode = table
        .get("mode")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            table
                .get("theme_type")
                .and_then(|v| v.as_str())
                .map(|value| {
                    if value.eq_ignore_ascii_case("light") {
                        "light".into()
                    } else {
                        "dark".into()
                    }
                })
        });
    RawColors {
        mode,
        background: pick(&["background", "bg"]),
        foreground: pick(&["foreground", "fg"]),
        accent: pick(&["accent", "blue", "color4"]),
        selection: pick(&["selection", "selection_background", "color0"]),
        muted: pick(&["muted", "dark_foreground", "dark_fg", "color8"]),
        red: pick(&["red", "color1"]),
        green: pick(&["green", "color2"]),
        yellow: pick(&["yellow", "color3"]),
        blue: pick(&["blue", "color4"]),
        darker_background: pick(&["darker_background", "darker_bg"]),
        lighter_background: pick(&["lighter_background", "lighter_bg"]),
        dark_background: pick(&["dark_background", "dark_bg"]),
        border: pick(&["border"]),
    }
}

fn raw_from_alacritty(text: &str) -> Option<RawColors> {
    let mut section = String::new();
    let mut map = std::collections::HashMap::<String, String>::new();
    for line in text.lines() {
        let line = strip_comment(line).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            if let Some(end) = header.find(']') {
                section = header[..end].trim().to_string();
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Some(color) = normalize_hex(value.trim()) else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let path = if section.is_empty() {
            key.to_string()
        } else {
            format!("{section}.{key}")
        };
        map.entry(path).or_insert(color);
    }
    let background = map
        .get("colors.primary.background")
        .cloned()
        .or_else(|| map.get("colors.normal.black").cloned())?;
    let foreground = map
        .get("colors.primary.foreground")
        .cloned()
        .or_else(|| map.get("colors.normal.white").cloned())?;
    Some(RawColors {
        background: Some(background),
        foreground: Some(foreground),
        accent: map.get("colors.normal.blue").cloned(),
        selection: map.get("colors.selection.background").cloned(),
        muted: map.get("colors.bright.black").cloned(),
        red: map.get("colors.normal.red").cloned(),
        green: map.get("colors.normal.green").cloned(),
        yellow: map.get("colors.normal.yellow").cloned(),
        blue: map.get("colors.normal.blue").cloned(),
        ..RawColors::default()
    })
}

fn raw_from_css(text: &str) -> Option<RawColors> {
    let mut raw = RawColors::default();
    let mut selector = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("@define-color") {
            apply_define(&mut raw, trimmed);
            continue;
        }
        if let Some(open) = trimmed.find('{') {
            selector = trimmed[..open].to_ascii_lowercase();
        }
        if trimmed.contains('}') {
            selector.clear();
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key
            .trim()
            .trim_start_matches('-')
            .trim_start_matches('-')
            .to_ascii_lowercase();
        let Some(color) = first_color(value) else {
            continue;
        };
        let selected = selector.contains("selected")
            || selector.contains("active")
            || selector.contains("focused")
            || selector.contains("focus");
        match key.as_str() {
            "background" | "background-color" | "window_bg" | "base" | "bg" if selected => {
                if raw.selection.is_none() {
                    raw.selection = Some(color);
                }
            }
            "background" | "background-color" | "window_bg" | "base" | "bg" => {
                if raw.background.is_none() {
                    raw.background = Some(color);
                }
            }
            "color" | "foreground" | "text" | "fg" if !selected => {
                if raw.foreground.is_none() {
                    raw.foreground = Some(color);
                }
            }
            "accent" | "selected" | "selected-text" => {
                if raw.accent.is_none() {
                    raw.accent = Some(color);
                }
            }
            "selection" | "selected_bg" | "selected-background" | "selection_background" => {
                if raw.selection.is_none() {
                    raw.selection = Some(color);
                }
            }
            "border" | "border-color" | "border_color" => {
                if raw.border.is_none() {
                    raw.border = Some(color);
                }
            }
            "muted" | "subtext" => {
                if raw.muted.is_none() {
                    raw.muted = Some(color);
                }
            }
            other if other.contains("border") && raw.accent.is_none() => {
                raw.accent = Some(color.clone());
                if raw.border.is_none() {
                    raw.border = Some(color);
                }
            }
            _ => {}
        }
    }
    if raw.background.is_some() && raw.foreground.is_some() {
        Some(raw)
    } else {
        None
    }
}

fn apply_define(raw: &mut RawColors, line: &str) {
    let rest = line.trim_start_matches("@define-color").trim();
    let mut parts = rest.split_whitespace();
    let Some(name) = parts.next() else {
        return;
    };
    let Some(value) = parts.next() else {
        return;
    };
    let Some(color) = first_color(value.trim_end_matches(';')) else {
        return;
    };
    let name = name.trim_end_matches(';').to_ascii_lowercase();
    let slot = match name.as_str() {
        "background" | "window_bg" | "base" | "bg" => &mut raw.background,
        "foreground" | "text" | "fg" | "color" => &mut raw.foreground,
        "accent" | "selected" => &mut raw.accent,
        "selection" | "selected_bg" | "selected-background" => &mut raw.selection,
        "border" | "border_color" | "border-color" => &mut raw.border,
        "muted" | "subtext" => &mut raw.muted,
        "red" => &mut raw.red,
        "green" => &mut raw.green,
        "yellow" => &mut raw.yellow,
        "blue" => &mut raw.blue,
        _ => return,
    };
    if slot.is_none() {
        *slot = Some(color);
    }
}

pub fn load_active_palette(home: &Path) -> Palette {
    let mut palette = resolve_theme_dir(home)
        .and_then(|dir| load_palette_path(&dir))
        .unwrap_or_else(Palette::fallback);
    palette.font = load_font_name(home);
    palette
}

/// Monospace family from the Omarchy font config.
///
/// `omarchy-font-set` writes `~/.config/fontconfig/fonts.conf`. Terminal
/// configs are the fallback when that file is absent.
pub fn load_font_name(home: &Path) -> String {
    if let Some(name) = read_font(
        &home.join(".config/fontconfig/fonts.conf"),
        fontconfig_monospace,
    ) {
        return name;
    }
    if let Some(name) = read_font(
        &home.join(".config/alacritty/alacritty.toml"),
        alacritty_font_family,
    ) {
        return name;
    }
    if let Some(name) = read_font(&home.join(".config/kitty/kitty.conf"), kitty_font_family) {
        return name;
    }
    if let Some(name) = read_font(&home.join(".config/ghostty/config"), ghostty_font_family) {
        return name;
    }
    if let Some(name) = read_font(&home.join(".config/foot/foot.ini"), foot_font_family) {
        return name;
    }
    DEFAULT_FONT.into()
}

fn read_font(path: &Path, parse: fn(&str) -> Option<String>) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let name = parse(&text)?;
    let name = name.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn fontconfig_monospace(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let marker = "<string>monospace</string>";
    let idx = lower.find(marker)?;
    let rest = &text[idx + marker.len()..];
    let start = rest.to_ascii_lowercase().find("<string>")? + "<string>".len();
    let end = rest[start..].to_ascii_lowercase().find("</string>")?;
    let name = rest[start..start + end].trim();
    if name.is_empty() || name.eq_ignore_ascii_case("monospace") {
        None
    } else {
        Some(name.to_string())
    }
}

fn alacritty_font_family(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = strip_comment(line).trim();
        let Some(rest) = line.strip_prefix("family") else {
            continue;
        };
        let rest = rest.trim();
        if !rest.starts_with('=') {
            continue;
        }
        let value = rest.trim_start_matches('=').trim();
        let value = value.trim_matches('"').trim_matches('\'');
        if !value.is_empty() && !value.starts_with('#') {
            return Some(value.to_string());
        }
    }
    None
}

fn kitty_font_family(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("font_family") else {
            continue;
        };
        let name = rest.trim();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

fn ghostty_font_family(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = strip_comment(line).trim();
        let Some(rest) = line.strip_prefix("font-family") else {
            continue;
        };
        let value = rest.trim().trim_start_matches('=').trim();
        let value = value.trim_matches('"').trim_matches('\'');
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

fn foot_font_family(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("font=") else {
            continue;
        };
        let name = rest.split(':').next().unwrap_or("").trim();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => in_string = !in_string,
            b'#' if !in_string => return &line[..index],
            _ => {}
        }
        index += 1;
    }
    line
}

fn first_color(value: &str) -> Option<String> {
    if let Some(start) = value.find('#') {
        if let Some(color) = normalize_hex(&value[start..]) {
            return Some(color);
        }
    }
    let lower = value.to_ascii_lowercase();
    let rgb_at = lower.find("rgb(").or_else(|| lower.find("rgba("))?;
    let open = value[rgb_at..].find('(')? + rgb_at;
    let close = value[open..].find(')')? + open;
    let parts: Vec<&str> = value[open + 1..close].split(',').collect();
    if parts.len() < 3 {
        return None;
    }
    let channel = |text: &str| -> Option<u8> {
        let text = text.trim();
        if text.ends_with('%') {
            let pct: f64 = text.trim_end_matches('%').trim().parse().ok()?;
            return Some((pct / 100.0 * 255.0).round().clamp(0.0, 255.0) as u8);
        }
        let number: f64 = text.parse().ok()?;
        Some(number.round().clamp(0.0, 255.0) as u8)
    };
    let r = channel(parts[0])?;
    let g = channel(parts[1])?;
    let b = channel(parts[2])?;
    Some(format!("#{r:02x}{g:02x}{b:02x}"))
}

fn normalize_hex(raw: &str) -> Option<String> {
    let text = raw
        .trim()
        .trim_matches(|c: char| c == '"' || c == '\'' || c == ';');
    let hex = text
        .strip_prefix('#')
        .or_else(|| text.strip_prefix("0x"))
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
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

fn channel_sum(hex: &str) -> u16 {
    let Some((r, g, b)) = rgb(hex) else {
        return 0;
    };
    u16::from(r) + u16::from(g) + u16::from(b)
}

fn with_alpha(hex: &str, alpha: f64) -> String {
    let Some((r, g, b)) = rgb(hex) else {
        return hex.to_string();
    };
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
}

fn contrast_fg(bg: &str) -> String {
    if luminance(bg) > 0.45 {
        "#1a1a1a".into()
    } else {
        "#ffffff".into()
    }
}

fn css_font_family(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "monospace".into()
    } else {
        format!("\"{cleaned}\", monospace")
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
        assert!(!css.contains(":root"));
        assert!(!css.contains("--window-"));
        assert!(!css.contains("--accent-"));
        assert!(!css.contains("--view-"));
        assert!(!css.contains("--headerbar-"));
        assert!(!css.contains("--sidebar-"));
        assert!(!css.contains("--card-"));
        assert!(!css.contains("--popover-"));
        assert!(!css.contains("--dialog-"));
        assert!(!css.contains("--destructive-"));
        assert!(!css.contains("--success-"));
        assert!(!css.contains("--warning-"));
        assert!(!css.contains("--error-"));
        assert!(css.contains("--omabring-bg: #1e1e2e;"));
        assert!(css.contains("--omabring-surface: #313244;"));
        assert!(css.contains("--omabring-accent: #89b4fa;"));
        assert!(css.contains("--omabring-border:"));
        assert!(css.contains("background-color: var(--omabring-bg);"));
        assert!(css.contains("border-radius: 0;"));
        assert!(css.contains("font-family: \"JetBrainsMono Nerd Font\", monospace;"));
        assert!(css.contains("color: var(--omabring-accent);"));
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
        let watched = theme_watch_paths(home.path());
        assert!(watched.iter().any(|path| path.ends_with("omarchy/current")));
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
        let watched = theme_watch_paths(home.path());
        assert!(watched.iter().any(|path| path == &state));
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

    #[test]
    fn colors_toml_overrides_alacritty_and_css() {
        let home = tempfile::tempdir().unwrap();
        let theme = home.path().join(".config/omarchy/current/theme");
        fs::create_dir_all(&theme).unwrap();
        fs::write(
            theme.join("alacritty.toml"),
            r##"
[colors.primary]
background = "#111111"
foreground = "#eeeeee"

[colors.normal]
black = "#111111"
red = "#aa0000"
green = "#00aa00"
yellow = "#aaaa00"
blue = "#0000aa"
magenta = "#aa00aa"
cyan = "#00aaaa"
white = "#eeeeee"

[colors.selection]
background = "#333333"
"##,
        )
        .unwrap();
        fs::write(
            theme.join("walker.css"),
            "@define-color background #222222;\n@define-color foreground #dddddd;\n@define-color accent #00ff00;\n@define-color border #444444;\n",
        )
        .unwrap();
        fs::write(
            theme.join("waybar.css"),
            "window#waybar { background-color: #010101; color: #fefefe; border-bottom: 2px solid #ff00aa; }\n#workspaces button.active { background-color: #123456; }\n",
        )
        .unwrap();
        let from_terminal = load_palette_path(&theme).unwrap();
        assert_eq!(from_terminal.background, "#111111");
        assert_eq!(from_terminal.accent, "#0000aa");
        assert_eq!(from_terminal.border, "#444444");
        fs::write(
            theme.join("colors.toml"),
            "background = \"#1a1b26\"\nforeground = \"#a9b1d6\"\naccent = \"#7aa2f7\"\n",
        )
        .unwrap();
        let palette = load_palette_path(&theme).unwrap();
        assert_eq!(palette.background, "#1a1b26");
        assert_eq!(palette.foreground, "#a9b1d6");
        assert_eq!(palette.accent, "#7aa2f7");
        assert_eq!(palette.border, "#444444");
    }

    #[test]
    fn alacritty_alone_is_enough() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("alacritty.toml"),
            r##"
[colors.primary]
background = "0x1a1b26"
foreground = "#a9b1d6"

[colors.normal]
blue = "#7aa2f7"
red = "#f7768e"
"##,
        )
        .unwrap();
        let palette = load_palette_path(dir.path()).unwrap();
        assert_eq!(palette.background, "#1a1b26");
        assert_eq!(palette.accent, "#7aa2f7");
        assert!(palette.dark);
    }

    #[test]
    fn walker_and_waybar_supply_a_palette() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("walker.css"),
            "@define-color window_bg #fffaf3;\n@define-color text #221100;\n@define-color accent #883300;\n@define-color selection #ecdcc8;\n",
        )
        .unwrap();
        let palette = load_palette_path(dir.path()).unwrap();
        assert_eq!(palette.background, "#fffaf3");
        assert_eq!(palette.foreground, "#221100");
        assert_eq!(palette.accent, "#883300");
        assert!(!palette.dark);
        fs::remove_file(dir.path().join("walker.css")).unwrap();
        fs::create_dir_all(dir.path().join("waybar")).unwrap();
        fs::write(
            dir.path().join("waybar/style.css"),
            "window#waybar {\n  background: rgb(10, 20, 30);\n  color: #abcdef;\n  border-bottom: 3px solid #ff8800;\n}\n",
        )
        .unwrap();
        let palette = load_palette_path(dir.path()).unwrap();
        assert_eq!(palette.background, "#0a141e");
        assert_eq!(palette.foreground, "#abcdef");
        assert_eq!(palette.accent, "#ff8800");
    }

    #[test]
    fn light_mode_file_marks_a_light_theme() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("light.mode"), "").unwrap();
        fs::write(
            dir.path().join("colors.toml"),
            "background = \"#101010\"\nforeground = \"#eeeeee\"\n",
        )
        .unwrap();
        let palette = load_palette_path(dir.path()).unwrap();
        assert!(!palette.dark);
        assert_eq!(palette.mode, "light");
    }

    #[test]
    fn font_comes_from_fontconfig_then_terminals() {
        let home = tempfile::tempdir().unwrap();
        let fonts = home.path().join(".config/fontconfig");
        fs::create_dir_all(&fonts).unwrap();
        fs::write(
            fonts.join("fonts.conf"),
            r#"<?xml version="1.0"?>
<fontconfig>
  <match target="pattern">
    <test name="family" qual="any"><string>monospace</string></test>
    <edit name="family" mode="prepend_first" binding="strong">
      <string>CaskaydiaMono Nerd Font</string>
    </edit>
  </match>
</fontconfig>
"#,
        )
        .unwrap();
        let palette = load_active_palette(home.path());
        assert_eq!(palette.font, "CaskaydiaMono Nerd Font");
        assert!(palette.to_css().contains("CaskaydiaMono Nerd Font"));
        fs::remove_file(fonts.join("fonts.conf")).unwrap();
        let alacritty = home.path().join(".config/alacritty");
        fs::create_dir_all(&alacritty).unwrap();
        fs::write(
            alacritty.join("alacritty.toml"),
            "family = \"Iosevka Term\"\n",
        )
        .unwrap();
        assert_eq!(load_font_name(home.path()), "Iosevka Term");
    }
}
