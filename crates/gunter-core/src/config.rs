use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub shell: ShellConfig,
    pub font: FontConfig,
    pub theme: ThemeConfig,
    pub scrollback: ScrollbackConfig,
    pub keys: KeysConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ShellConfig {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct FontConfig {
    pub family: String,
    pub fallback: Vec<String>,
    pub size: f32,
    pub line_height: f32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ScrollbackConfig {
    pub lines: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct KeysConfig {
    pub new_tab: String,
    pub close_tab: String,
    pub split_h: String,
    pub split_v: String,
    pub next_pane: String,
    pub prev_pane: String,
    pub zoom_pane: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            shell: ShellConfig::default(),
            font: FontConfig::default(),
            theme: ThemeConfig::default(),
            scrollback: ScrollbackConfig::default(),
            keys: KeysConfig::default(),
        }
    }
}

impl Default for ShellConfig {
    fn default() -> Self {
        let program = std::env::var("SHELL").unwrap_or_else(|_| {
            if cfg!(target_os = "windows") { "cmd.exe".to_string() }
            else { "/bin/bash".to_string() }
        });
        ShellConfig { program, args: vec![] }
    }
}

impl Default for FontConfig {
    fn default() -> Self {
        FontConfig {
            family: "ComicShannsMono Nerd Font Mono".to_string(),
            fallback: vec![],
            size: 14.0,
            line_height: 1.2,
        }
    }
}

impl Default for ThemeConfig {
    fn default() -> Self { ThemeConfig { name: "atom-one-dark".to_string() } }
}

impl Default for ScrollbackConfig {
    fn default() -> Self { ScrollbackConfig { lines: 5000 } }
}

impl Default for KeysConfig {
    fn default() -> Self {
        KeysConfig {
            new_tab:   "Ctrl+Shift+T".to_string(),
            close_tab: "Ctrl+Shift+W".to_string(),
            split_h:   "Ctrl+Shift+H".to_string(),
            split_v:   "Ctrl+Shift+V".to_string(),
            next_pane: "Ctrl+Tab".to_string(),
            prev_pane: "Ctrl+Shift+Tab".to_string(),
            zoom_pane: "Ctrl+Z".to_string(),
        }
    }
}

impl Config {
    /// Load from `~/.config/gunter/config.toml`, falling back to defaults.
    pub fn load() -> Self {
        let path = config_path();
        if let Some(p) = path {
            if let Ok(text) = std::fs::read_to_string(&p) {
                match toml::from_str::<Config>(&text) {
                    Ok(c) => return c,
                    Err(e) => eprintln!("config parse error: {e}"),
                }
            }
        }
        Config::default()
    }

    pub fn reload(&mut self) {
        *self = Config::load();
    }
}

pub fn config_path() -> Option<PathBuf> {
    dirs_next().map(|d| d.join("gunter").join("config.toml"))
}

fn dirs_next() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA").ok().map(PathBuf::from)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var("XDG_CONFIG_HOME")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config"))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_expected_values() {
        let c = Config::default();
        assert_eq!(c.font.size, 14.0);
        assert_eq!(c.scrollback.lines, 5000);
        assert_eq!(c.theme.name, "atom-one-dark");
    }

    #[test]
    fn config_parses_toml() {
        let toml = r#"
[font]
size = 18.0
family = "Fira Code"

[scrollback]
lines = 2000
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.font.size, 18.0);
        assert_eq!(c.scrollback.lines, 2000);
    }
}
