use std::fs;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct NetworkMetrics {
    pub interface: String,
    pub rx_speed_kb: f32,
    pub tx_speed_kb: f32,
    #[allow(dead_code)]
    pub is_connected: bool,
}

impl Default for NetworkMetrics {
    fn default() -> Self {
        Self {
            interface: "wlan0".to_string(),
            rx_speed_kb: 0.0,
            tx_speed_kb: 0.0,
            is_connected: true,
        }
    }
}

pub struct NetworkMonitor {
    prev_rx_bytes: u64,
    prev_tx_bytes: u64,
    prev_time: Option<Instant>,
}

impl NetworkMonitor {
    pub fn new() -> Self {
        Self {
            prev_rx_bytes: 0,
            prev_tx_bytes: 0,
            prev_time: None,
        }
    }

    pub fn sample(&mut self) -> NetworkMetrics {
        let now = Instant::now();
        let (interface, current_rx, current_tx) = Self::read_net_dev();

        let (rx_speed, tx_speed) = if let Some(prev) = self.prev_time {
            let elapsed = now.duration_since(prev).as_secs_f32().max(0.1);
            let rx_diff = current_rx.saturating_sub(self.prev_rx_bytes);
            let tx_diff = current_tx.saturating_sub(self.prev_tx_bytes);

            let rx_kb = (rx_diff as f32 / 1024.0) / elapsed;
            let tx_kb = (tx_diff as f32 / 1024.0) / elapsed;
            (rx_kb, tx_kb)
        } else {
            (0.0, 0.0)
        };

        self.prev_rx_bytes = current_rx;
        self.prev_tx_bytes = current_tx;
        self.prev_time = Some(now);

        let is_connected = current_rx > 0 || current_tx > 0;

        NetworkMetrics {
            interface,
            rx_speed_kb: rx_speed,
            tx_speed_kb: tx_speed,
            is_connected,
        }
    }

    fn read_net_dev() -> (String, u64, u64) {
        if let Ok(content) = fs::read_to_string("/proc/net/dev") {
            let mut best_interface = "Ağ Yok".to_string();
            let mut max_rx = 0u64;
            let mut total_rx = 0u64;
            let mut total_tx = 0u64;

            for line in content.lines().skip(2) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.is_empty() {
                    continue;
                }
                let iface_raw = parts[0].trim_end_matches(':');
                if iface_raw == "lo" {
                    continue;
                }

                let rx: u64 = parts.get(1).and_then(|v| v.parse().ok()).unwrap_or(0);
                let tx: u64 = parts.get(9).and_then(|v| v.parse().ok()).unwrap_or(0);

                total_rx += rx;
                total_tx += tx;

                if rx > max_rx {
                    max_rx = rx;
                    best_interface = iface_raw.to_string();
                }
            }

            if best_interface == "Ağ Yok" && total_rx > 0 {
                best_interface = "eth0".to_string();
            }

            return (best_interface, total_rx, total_tx);
        }

        ("Bilinmiyor".to_string(), 0, 0)
    }

    pub fn format_rate(kb: f32) -> String {
        if kb >= 1024.0 {
            format!("{:.1} MB/s", kb / 1024.0)
        } else {
            format!("{:.0} KB/s", kb)
        }
    }
}
