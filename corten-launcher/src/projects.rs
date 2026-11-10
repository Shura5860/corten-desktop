use std::path::{Path, PathBuf};
use tracing::info;

#[derive(Debug, Clone)]
pub struct DevProject {
    pub name: String,
    pub path: PathBuf,
    pub has_git: bool,
    pub project_type: &'static str, // "Rust", "Node", "Python", "Go", "Git"
    pub git_branch: Option<String>,
    pub git_dirty_count: Option<usize>,
}

impl DevProject {
    pub fn open_in_editor(&self) -> std::io::Result<()> {
        let path_str = self.path.to_string_lossy();
        info!("Opening developer project at {}", path_str);

        // Try user preferred editor, then common IDEs, then fallback terminal editor
        let editors = ["code", "cursor", "zed", "subl", "alacritty", "kitty"];
        for ed in editors {
            if let Ok(child) = std::process::Command::new(ed).arg(&*path_str).spawn() {
                let _ = child.id();
                return Ok(());
            }
        }

        // Fallback: pass the directory directly to a terminal/editor.
        for terminal in [
            "corten-terminal",
            "/usr/local/bin/corten-terminal",
            "target/release/corten-terminal",
            "target/debug/corten-terminal",
            "alacritty",
            "kitty",
            "foot",
            "xterm",
        ] {
            let mut command = std::process::Command::new(terminal);
            command.arg("-e").arg("nvim").arg(&*path_str);
            if command.spawn().is_ok() {
                return Ok(());
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no supported terminal/editor found",
        ))
    }

    pub fn open_in_terminal(&self) -> std::io::Result<()> {
        for terminal in [
            "corten-terminal",
            "/usr/local/bin/corten-terminal",
            "/usr/bin/corten-terminal",
            "target/release/corten-terminal",
            "target/debug/corten-terminal",
            "foot",
            "alacritty",
            "kitty",
            "konsole",
            "xterm",
            "pterm",
        ] {
            if std::process::Command::new(terminal)
                .current_dir(&self.path)
                .spawn()
                .is_ok()
            {
                return Ok(());
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no supported terminal found",
        ))
    }

    pub fn open_in_files(&self) -> std::io::Result<()> {
        let path_str = self.path.to_string_lossy();
        for files_app in [
            "corten-files",
            "/usr/local/bin/corten-files",
            "/usr/bin/corten-files",
            "target/release/corten-files",
            "target/debug/corten-files",
        ] {
            if std::process::Command::new(files_app)
                .arg(&*path_str)
                .spawn()
                .is_ok()
            {
                return Ok(());
            }
        }
        let _ = std::process::Command::new("xdg-open").arg(&*path_str).spawn();
        Ok(())
    }
}

/// Discovers developer repositories in standard directories (~/Projects, ~/code, ~/src, ~/Workspace, ~/ghq)
pub fn scan_dev_projects() -> Vec<DevProject> {
    let mut projects = Vec::new();
    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => return projects,
    };

    let search_roots = [
        home.join("Projects"),
        home.join("projects"),
        home.join("code"),
        home.join("Code"),
        home.join("src"),
        home.join("Workspace"),
        home.join("ghq"),
    ];

    for root in search_roots {
        if !root.is_dir() {
            continue;
        }

        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with('.') {
                        continue;
                    }

                    let has_git = path.join(".git").exists();
                    let (git_branch, git_dirty_count) = if has_git {
                        detect_git_info(&path)
                    } else {
                        (None, None)
                    };
                    let project_type = detect_project_type(&path);

                    projects.push(DevProject {
                        name,
                        path,
                        has_git,
                        project_type,
                        git_branch,
                        git_dirty_count,
                    });
                }
            }
        }
    }

    projects
}

fn detect_git_info(repo_path: &Path) -> (Option<String>, Option<usize>) {
    let head_path = repo_path.join(".git").join("HEAD");
    let branch = if let Ok(content) = std::fs::read_to_string(&head_path) {
        let trimmed = content.trim();
        if let Some(ref_name) = trimmed.strip_prefix("ref: refs/heads/") {
            Some(ref_name.to_string())
        } else if trimmed.len() >= 7 {
            Some(trimmed[..7].to_string())
        } else {
            None
        }
    } else {
        None
    };

    let dirty_count = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repo_path)
        .output()
        .ok()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count()
        });

    (branch, dirty_count)
}

fn detect_project_type(path: &Path) -> &'static str {
    if path.join("Cargo.toml").exists() {
        "Rust"
    } else if path.join("package.json").exists() {
        "Node.js"
    } else if path.join("go.mod").exists() {
        "Go"
    } else if path.join("pyproject.toml").exists() || path.join("requirements.txt").exists() {
        "Python"
    } else if path.join(".git").exists() {
        "Git Repo"
    } else {
        "Project"
    }
}
