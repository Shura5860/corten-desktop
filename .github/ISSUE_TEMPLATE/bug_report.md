---
name: Bug Report
about: Create a report to help us improve Corten Desktop
title: '[BUG] '
labels: 'bug'
assignees: ''

---

**Describe the bug**
A clear and concise description of what the bug is.

**Hardware & System Information:**
- **OS:** (e.g. Arch Linux, Fedora 40, Ubuntu 24.04)
- **Kernel Version:** `uname -r`
- **GPU & Driver:** (e.g. AMD RX 6700XT Mesa 24.1 / NVIDIA RTX 3070 Driver 555.58)
- **Multi-Monitor Setup:** (e.g. Dual-monitor, DP-1 2560x1440@144Hz, HDMI-1 1920x1080@60Hz)
- **Session Type:** (Bare-metal TTY via `corten-session` or Nested Window under GNOME/KDE)

**To Reproduce**
Steps to reproduce the behavior:
1. Launch '...'
2. Press '...'
3. See error

**Expected behavior**
A clear and concise description of what you expected to happen.

**Logs & Diagnostic Output**
Run Corten with debug logs and attach the relevant snippet:
```bash
RUST_LOG=corten_compositor=debug,smithay=info corten-compositor
```

