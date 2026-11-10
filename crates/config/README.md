# Corten Config (`corten-config`)

Corten Masaüstü Ortamı bileşenleri için ortak TOML yapılandırma yükleyicisi ve canlı dosya izleyicisi (`notify`).

## Özellikler
- **Standart XDG Yolu:** Doğrudan `~/.config/corten/<bileşen>.toml` yollarını yönetir.
- **Otomatik Başlangıç:** Yapılandırma dosyası yoksa varsayılan (`Default`) değerlerle dosyayı otomatik oluşturur.
- **Canlı İzleme (`live-reload`):** `spawn_watcher()` metoduyla arka planda dosya değişikliklerini dinler ve güncel yapılandırmayı kanal üzerinden yayınlar.
- **Tip Güvenli:** `serde` tabanlı güçlü serileştirme/ayrıştırma.

## Kullanım

```rust
use corten_config::ConfigManager;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct PanelConfig {
    pub position_top: bool,
    pub height: u32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ~/.config/corten/panel.toml yüklenir veya oluşturulur
    let manager = ConfigManager::<PanelConfig>::load_or_default("panel")?;
    println!("Panel yüksekliği: {}", manager.get().height);

    // Canlı izleyici başlatma
    let (_watcher, rx) = manager.spawn_watcher()?;
    // rx.recv() ile güncellemeleri dinleyin...

    Ok(())
}
```
