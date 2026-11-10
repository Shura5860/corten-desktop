use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    pub name: String,
    pub exec: Vec<String>,
    pub is_wayland: bool,
}

pub struct SessionManager;

impl SessionManager {
    /// Discovers all available Wayland and X11 sessions installed on system
    pub fn discover_sessions() -> Vec<SessionEntry> {
        let mut sessions = Vec::new();

        // 1. Wayland sessions (/usr/share/wayland-sessions)
        if let Ok(entries) = fs::read_dir("/usr/share/wayland-sessions") {
            for entry in entries.flatten() {
                if let Some(session) = Self::parse_desktop_file(&entry.path(), true) {
                    sessions.push(session);
                }
            }
        }

        // 2. X11 sessions fallback (/usr/share/xsessions)
        if let Ok(entries) = fs::read_dir("/usr/share/xsessions") {
            for entry in entries.flatten() {
                if let Some(session) = Self::parse_desktop_file(&entry.path(), false) {
                    sessions.push(session);
                }
            }
        }

        if sessions.is_empty() {
            // Default Corten session fallback
            sessions.push(SessionEntry {
                name: "Corten Desktop".into(),
                exec: vec!["corten-compositor".into()],
                is_wayland: true,
            });
        }

        sessions
    }

    fn parse_desktop_file(path: &PathBuf, is_wayland: bool) -> Option<SessionEntry> {
        let content = fs::read_to_string(path).ok()?;
        let mut name = None;
        let mut exec = None;

        for line in content.lines() {
            let line = line.trim();
            if line.starts_with("Name=") && name.is_none() {
                name = Some(line.trim_start_matches("Name=").to_string());
            } else if line.starts_with("Exec=") && exec.is_none() {
                exec = Some(line.trim_start_matches("Exec=").to_string());
            }
        }

        let exec = parse_exec(&exec.unwrap_or_else(|| "default".to_string())).ok()?;

        Some(SessionEntry {
            name: name.unwrap_or_else(|| {
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            }),
            exec,
            is_wayland,
        })
    }
}

fn parse_exec(value: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;

    for ch in value.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' if quote != Some('\'') => escaped = true,
            '\'' | '"' if quote == Some(ch) => quote = None,
            '\'' | '"' if quote.is_none() => quote = Some(ch),
            c if c.is_whitespace() && quote.is_none() => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if escaped || quote.is_some() {
        return Err("unterminated escape or quote".into());
    }
    if !current.is_empty() {
        args.push(current);
    }

    let mut result = Vec::new();
    for arg in args {
        if let Some(code) = arg.strip_prefix('%') {
            match code {
                "%" => result.push("%".into()),
                "f" | "F" | "u" | "U" | "i" | "c" | "k" => continue,
                _ => return Err(format!("unsupported field code %{code}")),
            }
        } else {
            result.push(arg);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::parse_exec;

    #[test]
    fn parses_session_exec_arguments() {
        assert_eq!(
            parse_exec("corten-session --profile \"safe mode\" %u").unwrap(),
            vec!["corten-session", "--profile", "safe mode"]
        );
    }
}
