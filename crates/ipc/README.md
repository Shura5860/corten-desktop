# Corten IPC (`corten-ipc`)

Corten Masaüstü Ortamı için ortak D-Bus (`zbus`) arayüz tanımları, proxy trait'leri ve CLI tetikleme yardımcıları.

## Desteklenen Arayüzler
- **`org.corten.Launcher` (`LauncherControlProxy`):**
  - `toggle()`: Başlatıcıyı açar/kapatır (Sway/Niri kısayolları için).
  - `open()`, `close()`.
- **`org.corten.Notification` (`NotificationControlProxy`):**
  - `toggle_dnd()`: Rahatsız etme modunu açar/kapatır.
  - `clear_all()`: Ekrandaki bildirimleri temizler.
- **`org.corten.Panel` (`PanelControlProxy`):**
  - `toggle_visibility()`: Paneli gizler/gösterir.
  - `trigger_module(name)`: İlgili panel modülünü (ör. takvim, ses, pano) tetikler.

## Kullanım Örneği (CLI Tetikleyici)

```rust
use corten_ipc::{get_session_connection, LauncherControlProxy};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let connection = get_session_connection().await?;
    let launcher = LauncherControlProxy::new(&connection).await?;
    
    // Kısayol tuşuna basıldığında başlatıcıyı tetikle
    launcher.toggle().await?;
    Ok(())
}
```
