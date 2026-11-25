use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    pub name: String,
    pub exec: Vec<String>,
    pub is_wayland: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserAccount {
    pub username: String,
    pub home_dir: String,
}

pub struct SessionManager;

impl SessionManager {
    /// Discovers all available Wayland and X11 `.desktop` sessions installed on the system,
    /// placing Corten Desktop first when present.
    pub fn discover_sessions() -> Vec<SessionEntry> {
        let mut sessions = Vec::new();

        // 1. Wayland sessions (/usr/share/wayland-sessions)
        Self::collect_dir("/usr/share/wayland-sessions", true, &mut sessions);
        Self::collect_dir("/usr/local/share/wayland-sessions", true, &mut sessions);

        // 2. X11 sessions fallback (/usr/share/xsessions)
        Self::collect_dir("/usr/share/xsessions", false, &mut sessions);

        // Deduplicate by (name, is_wayland)
        let mut unique: Vec<SessionEntry> = Vec::new();
        for entry in sessions {
            if !unique
                .iter()
                .any(|u| u.name == entry.name && u.is_wayland == entry.is_wayland)
            {
                unique.push(entry);
            }
        }

        if unique.is_empty() {
            unique.push(SessionEntry {
                name: "Corten Desktop".into(),
                exec: vec!["corten-session".into()],
                is_wayland: true,
            });
        }

        // Deterministic sort: Corten first, then Wayland sessions alphabetically, then X11.
        unique.sort_by(|a, b| {
            let a_corten = a.name.to_ascii_lowercase().contains("corten");
            let b_corten = b.name.to_ascii_lowercase().contains("corten");
            b_corten
                .cmp(&a_corten)
                .then_with(|| b.is_wayland.cmp(&a.is_wayland))
                .then_with(|| {
                    a.name
                        .to_ascii_lowercase()
                        .cmp(&b.name.to_ascii_lowercase())
                })
        });

        unique
    }

    fn collect_dir(dir: &str, is_wayland: bool, out: &mut Vec<SessionEntry>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("desktop")
                    && let Some(session) = Self::parse_desktop_file(&path, is_wayland)
                {
                    out.push(session);
                }
            }
        }
    }

    fn parse_desktop_file(path: &Path, is_wayland: bool) -> Option<SessionEntry> {
        let content = fs::read_to_string(path).ok()?;
        let mut in_desktop_entry = false;
        let mut name = None;
        let mut exec = None;

        for raw_line in content.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                in_desktop_entry = line == "[Desktop Entry]";
                continue;
            }
            if !in_desktop_entry {
                continue;
            }
            if let Some(val) = line.strip_prefix("Hidden=")
                && val.trim().eq_ignore_ascii_case("true")
            {
                return None;
            }
            if let Some(val) = line.strip_prefix("NoDisplay=")
                && val.trim().eq_ignore_ascii_case("true")
            {
                return None;
            }
            if let Some(val) = line.strip_prefix("Name=")
                && name.is_none()
            {
                name = Some(val.trim().to_string());
            } else if let Some(val) = line.strip_prefix("Exec=")
                && exec.is_none()
            {
                exec = Some(val.trim().to_string());
            }
        }

        let exec_raw = exec?;
        let exec_args = parse_exec(&exec_raw).ok()?;
        if exec_args.is_empty() {
            return None;
        }

        Some(SessionEntry {
            name: name.unwrap_or_else(|| {
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            }),
            exec: exec_args,
            is_wayland,
        })
    }

    /// Discovers real human login users from `/etc/passwd` (`UID >= 1000 && UID < 65534`)
    /// so the greeter never pre-fills `"greeter"` when running under `greetd`.
    pub fn discover_human_users() -> Vec<UserAccount> {
        let Ok(content) = fs::read_to_string("/etc/passwd") else {
            return Vec::new();
        };
        parse_passwd_users(&content)
    }
}

pub(crate) fn parse_passwd_users(content: &str) -> Vec<UserAccount> {
    let mut users = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = trimmed.split(':').collect();
        if parts.len() < 7 {
            continue;
        }
        let username = parts[0];
        let Ok(uid) = parts[2].parse::<u32>() else {
            continue;
        };
        let home_dir = parts[5];
        let shell = parts[6];

        if !(1000..65534).contains(&uid) {
            continue;
        }
        if username == "nobody" || username == "greeter" {
            continue;
        }
        if shell.ends_with("/nologin") || shell.ends_with("/false") || shell.is_empty() {
            continue;
        }
        users.push(UserAccount {
            username: username.to_string(),
            home_dir: home_dir.to_string(),
        });
    }
    users
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
    use super::{parse_exec, parse_passwd_users};

    #[test]
    fn parses_session_exec_arguments() {
        assert_eq!(
            parse_exec("corten-session --profile \"safe mode\" %u").unwrap(),
            vec!["corten-session", "--profile", "safe mode"]
        );
    }

    #[test]
    fn filters_human_users_and_excludes_greeter() {
        let passwd = "\
root:x:0:0:root:/root:/bin/bash
greeter:x:966:966:Greeter:/var/lib/greetd:/usr/bin/nologin
shura:x:1000:1000:Shura:/home/shura:/bin/zsh
nobody:x:65534:65534:Nobody:/:/usr/bin/nologin
service:x:1001:1001:Svc:/var/lib/svc:/usr/bin/nologin
alice:x:1002:1002:Alice:/home/alice:/bin/bash
";
        let users = parse_passwd_users(passwd);
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].username, "shura");
        assert_eq!(users[1].username, "alice");
    }
}
