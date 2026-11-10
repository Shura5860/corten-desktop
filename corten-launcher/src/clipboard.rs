use std::io::Write;
use std::process::{Command, Stdio};
use tracing::debug;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardEntry {
    pub raw: String,
    pub preview: String,
    pub is_image: bool,
}

pub struct ClipboardManager;

impl ClipboardManager {
    /// Checks if cliphist and wl-copy are available on the system.
    pub fn is_available() -> bool {
        Command::new("cliphist")
            .arg("-v")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
            && Command::new("wl-copy")
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
    }

    /// Fetches the list of clipboard items from `cliphist list`.
    pub fn list_entries() -> Result<Vec<ClipboardEntry>, String> {
        let output = Command::new("cliphist")
            .arg("list")
            .output()
            .map_err(|e| format!("cliphist list failed: {e}"))?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(format!("cliphist exited with error: {err}"));
        }

        let text = String::from_utf8_lossy(&output.stdout);
        let entries: Vec<ClipboardEntry> = text
            .lines()
            .map(|line| {
                // cliphist list outputs: "<id>\t<preview>"
                let preview = line.split('\t').nth(1).unwrap_or(line).trim().to_string();
                let is_image = preview.starts_with("[[ binary data") && (preview.contains("png") || preview.contains("jpg") || preview.contains("jpeg") || preview.contains("image"));
                let display_preview = if is_image {
                    format!("📷 {}", preview.trim_start_matches("[[").trim_end_matches("]]").trim())
                } else {
                    preview
                };

                ClipboardEntry {
                    raw: line.to_string(),
                    preview: display_preview,
                    is_image,
                }
            })
            .collect();

        debug!("Loaded {} clipboard entries from cliphist", entries.len());
        Ok(entries)
    }

    /// Decodes and copies the selected entry back into Wayland clipboard via `cliphist decode | wl-copy`.
    pub fn copy_to_clipboard(entry: &ClipboardEntry) -> Result<(), String> {
        let mut decode = Command::new("cliphist")
            .arg("decode")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn cliphist decode: {e}"))?;

        if let Some(mut stdin) = decode.stdin.take() {
            stdin
                .write_all(entry.raw.as_bytes())
                .map_err(|e| format!("Failed to pipe entry to cliphist decode: {e}"))?;
        }

        let decode_output = decode
            .wait_with_output()
            .map_err(|e| format!("cliphist decode failed: {e}"))?;

        let mut wl_cmd = Command::new("wl-copy");
        if entry.is_image {
            wl_cmd.args(["--type", "image/png"]);
        }

        let mut wl_copy = wl_cmd
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn wl-copy: {e}"))?;

        if let Some(mut stdin) = wl_copy.stdin.take() {
            stdin
                .write_all(&decode_output.stdout)
                .map_err(|e| format!("Failed to write to wl-copy: {e}"))?;
        }

        let status = wl_copy
            .wait()
            .map_err(|e| format!("wl-copy execution failed: {e}"))?;

        if !status.success() {
            return Err("wl-copy returned non-zero exit status".to_string());
        }

        Ok(())
    }
}
