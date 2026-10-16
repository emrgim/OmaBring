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
        let border = self.border.unwrap_or_else(|| {
            if !lighter_background.eq_ignore_ascii_case(&background) {
                lighter_background.clone()
            } else {
                muted.clone()
            }
        });
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
            border: "#313244".into(),
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
        render_sheet(self)
    }
}

fn render_sheet(palette: &Palette) -> String {
    let accent_fg = readable_fg(&palette.accent, &palette.background, &palette.foreground);
    let font = css_font_family(&palette.font);
    include_str!("../../../packaging/omabring.css.tpl")
        .replace("{{ background }}", &palette.background)
        .replace("{{ foreground }}", &palette.foreground)
        .replace("{{ accent_fg }}", &accent_fg)
        .replace("{{ accent }}", &palette.accent)
        .replace("{{ selection }}", &palette.selection)
        .replace("{{ muted }}", &palette.muted)
        .replace("{{ darker_background }}", &palette.darker_background)
        .replace("{{ lighter_background }}", &palette.lighter_background)
        .replace("{{ border }}", &palette.border)
        .replace("{{ red }}", &palette.red)
        .replace("{{ green }}", &palette.green)
        .replace("{{ yellow }}", &palette.yellow)
        .replace("{{ font }}", &font)
}

/// Stylesheet for the open window.
///
/// Omarchy writes one CSS file per app into the active theme when
/// `omarchy-theme-set` renders `~/.config/omarchy/themed/*.tpl`. A finished
/// `omabring.css` is the window sheet. `supermarket.css` is used when that
/// file is an OmaBring sheet. A template that still contains `{{`
/// placeholders is ignored, and the sheet is built from the palette.
/// `cycle.css` and `shader-desk.css` belong to other apps and are not applied.
pub fn active_stylesheet(home: &Path) -> String {
    if let Some(dir) = resolve_theme_dir(home) {
        for name in ["omabring.css", "supermarket.css"] {
            if let Some(sheet) = owned_sheet(&dir.join(name)) {
                return sheet;
            }
        }
    }
    load_active_palette(home).to_css()
}

fn owned_sheet(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    if !text.contains("window.omabring")
        || text.contains("{{")
        || text.contains("@define-color")
        || text.contains(":root")
        || text.contains("--window-")
        || text.contains("--accent-color")
    {
        return None;
    }
    Some(text)
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
    // Per-app sheets rendered into the theme. Later files win, so a grocery
    // sheet can name the accent and the border after colors.toml.
    for name in ["omabring.css", "supermarket.css"] {
        let file = dir.join(name);
        if file.is_file() {
            if let Ok(text) = fs::read_to_string(&file) {
                raw.overlay(raw_from_role_lines(&text));
            }
        }
    }
    if raw.mode.is_none() && dir.join("light.mode").is_file() {
        raw.mode = Some("light".into());
    }
    raw.finish()
}

fn raw_from_role_lines(text: &str) -> RawColors {
    let mut raw = RawColors::default();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("@define-color") {
            apply_define(&mut raw, trimmed);
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("--") else {
            continue;
        };
        let Some((key, value)) = rest.split_once(':') else {
            continue;
        };
        let key = key
            .trim()
            .trim_start_matches("omabring-")
            .to_ascii_lowercase();
        let Some(color) = first_color(value) else {
            continue;
        };
        assign_role(&mut raw, &key, color);
    }
    raw
}

fn assign_role(raw: &mut RawColors, key: &str, color: String) {
    let slot = match key {
        "background" | "bg" => &mut raw.background,
        "foreground" | "fg" => &mut raw.foreground,
        "accent" => &mut raw.accent,
        "selection" => &mut raw.selection,
        "muted" => &mut raw.muted,
        "border" | "border-color" => &mut raw.border,
        "lighter_background" | "lighter-background" => &mut raw.lighter_background,
        "darker_background" | "darker-background" => &mut raw.darker_background,
        "red" => &mut raw.red,
        "green" => &mut raw.green,
        "yellow" => &mut raw.yellow,
        "blue" => &mut raw.blue,
        _ => return,
    };
    *slot = Some(color);
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
        "lighter_background" | "lighter-background" | "lighter_bg" => &mut raw.lighter_background,
        "darker_background" | "darker-background" | "darker_bg" => &mut raw.darker_background,
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

fn readable_fg(accent: &str, background: &str, foreground: &str) -> String {
    let on_bg = contrast_ratio(accent, background);
    let on_fg = contrast_ratio(accent, foreground);
    if on_bg >= on_fg {
        background.to_string()
    } else {
        foreground.to_string()
    }
}

fn contrast_ratio(a: &str, b: &str) -> f64 {
    let l1 = luminance(a);
    let l2 = luminance(b);
    let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

fn css_font_family(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "\"IBM Plex Mono\", monospace".into()
    } else {
        format!("\"IBM Plex Mono\", \"{cleaned}\", monospace")
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
        assert!(css.contains("--omabring-fill: #89b4fa;"));
        assert!(css.contains("--omabring-on-fill: #1e1e2e;"));
        assert!(css.contains("--omabring-line: #313244;"));
        assert!(css.contains("--omabring-border: #313244;"));
        assert!(css.contains("background-color: var(--omabring-selection);"));
        assert!(css.contains("background-color: var(--omabring-bg);"));
        assert!(css.contains("border-radius: 0;"));
        assert!(
            css.contains("font-family: \"IBM Plex Mono\", \"JetBrainsMono Nerd Font\", monospace;")
        );
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
    fn resolves_state_theme_when_config_current_is_absent() {
        let home = tempfile::tempdir().unwrap();
        let state = home.path().join(".local/state/omarchy/current/theme");
        fs::create_dir_all(&state).unwrap();
        fs::write(
            state.join("colors.toml"),
            "background = \"#abcdef\"\nforeground = \"#111111\"\n",
        )
        .unwrap();
        let dir = resolve_theme_dir_at(home.path(), None).unwrap();
        assert_eq!(dir, state);
        let palette = load_palette_path(&dir).unwrap();
        assert_eq!(palette.background, "#abcdef");
        let watched = theme_watch_paths(home.path());
        assert!(watched.iter().any(|path| path == &state));
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

    #[test]
    fn accent_text_is_the_readable_theme_color() {
        let text = r##"
mode = "dark"
accent = "#88a4d2"
selection = "#2a2d3a"
muted = "#8b8d97"
background = "#151518"
darker_background = "#101014"
lighter_background = "#1c1c21"
foreground = "#e2e3e8"
"##;
        let palette = load_palette_str(text).unwrap();
        assert_eq!(palette.border, "#1c1c21");
        let css = palette.to_css();
        assert!(css.contains("--omabring-accent: #88a4d2;"));
        assert!(css.contains("--omabring-fill: #88a4d2;"));
        assert!(css.contains("--omabring-on-fill: #151518;"));
        assert!(css.contains("--omabring-line: #1c1c21;"));
        assert!(css.contains("--omabring-fg: #e2e3e8;"));
        assert!(!css.contains("#e2e3e866"));
    }

    #[test]
    fn border_uses_muted_when_the_raised_surface_matches_the_background() {
        let text = r##"
background = "#151518"
foreground = "#e2e3e8"
lighter_background = "#151518"
muted = "#8b8d97"
accent = "#88a4d2"
"##;
        let palette = load_palette_str(text).unwrap();
        assert_eq!(palette.border, "#8b8d97");
    }

    #[test]
    fn rendered_app_sheet_wins_and_a_template_does_not() {
        let home = tempfile::tempdir().unwrap();
        let theme = home.path().join(".local/state/omarchy/current/theme");
        fs::create_dir_all(&theme).unwrap();
        fs::write(
            theme.join("colors.toml"),
            "background = \"#151518\"\nforeground = \"#e2e3e8\"\naccent = \"#88a4d2\"\n",
        )
        .unwrap();
        fs::write(
            theme.join("supermarket.css"),
            "window.omabring { --omabring-accent: #112233; background: {{ accent }}; }\n",
        )
        .unwrap();
        let generated = active_stylesheet(home.path());
        assert!(generated.contains("--omabring-accent: #88a4d2;"));
        assert!(!generated.contains("{{"));
        fs::write(
            theme.join("omabring.css"),
            "window.omabring { background-color: #151518; color: #e2e3e8; }\n",
        )
        .unwrap();
        let sheet = active_stylesheet(home.path());
        assert!(sheet.contains("background-color: #151518;"));
        assert!(!sheet.contains("--omabring-fill"));
    }

    #[test]
    fn supermarket_role_colors_overlay_the_palette() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("colors.toml"),
            "background = \"#151518\"\nforeground = \"#e2e3e8\"\naccent = \"#88a4d2\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("supermarket.css"),
            "@define-color border #2a2a30;\n--accent: #88a4d2;\n",
        )
        .unwrap();
        let palette = load_palette_path(dir.path()).unwrap();
        assert_eq!(palette.accent, "#88a4d2");
        assert_eq!(palette.border, "#2a2a30");
    }
}
