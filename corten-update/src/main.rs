use std::time::Duration;
use tracing::info;

use corten_update::{CortenUpdater, CURRENT_VERSION};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(|s| s.as_str()).unwrap_or("check");

    match command {
        "check" | "--check" | "-c" => {
            println!("Corten Desktop OTA Güncelleme Kontrolü");
            println!("--------------------------------------------------");
            println!("Mevcut Sürüm : v{}", CURRENT_VERSION);

            match CortenUpdater::check_for_updates().await {
                Ok(info) => {
                    println!("En Son Sürüm : {}", info.latest_version);
                    println!("Yayın Tarihi : {}", info.published_at);
                    if let Some(bytes) = info.asset_size_bytes {
                        println!("Paket Boyutu : {:.1} MB", bytes as f64 / (1024.0 * 1024.0));
                    }
                    println!("--------------------------------------------------");
                    if info.has_update {
                        println!("★ YENİ BİR GÜNCELLEME MEVCUT!");
                        println!("\nDeğişiklik Notları:\n{}", info.release_notes);
                        println!("\nGüncellemeyi uygulamak için: corten-update apply");
                    } else {
                        println!("✓ Sisteminiz güncel.");
                    }
                }
                Err(err) => {
                    eprintln!("Güncelleme kontrolü başarısız: {}", err);
                }
            }
        }
        "apply" | "--apply" | "-a" | "upgrade" => {
            println!("Corten Desktop Güncelleme Uygulanıyor...");
            println!("--------------------------------------------------");

            match CortenUpdater::check_for_updates().await {
                Ok(info) => {
                    if !info.has_update && !args.iter().any(|a| a == "--force") {
                        println!("✓ Sisteminiz zaten en son sürümde ({}).", info.current_version);
                        println!("Yine de yeniden kurmak için: corten-update apply --force");
                        return Ok(());
                    }

                    println!("Yeni Sürüm : {}", info.latest_version);
                    let res = CortenUpdater::apply_update(&info, |msg| {
                        println!("==> {}", msg);
                    })
                    .await;

                    match res {
                        Ok(()) => println!("\n✓ Güncelleme başarıyla tamamlandı!"),
                        Err(e) => eprintln!("\n✗ Güncelleme başarısız: {}", e),
                    }
                }
                Err(err) => {
                    eprintln!("Güncelleme bilgisi alınamadı: {}", err);
                }
            }
        }
        "daemon" | "--daemon" => {
            info!("Corten OTA Update Daemon başlatıldı (periyot: 4 saat)...");
            loop {
                if let Ok(info) = CortenUpdater::check_for_updates().await {
                    if info.has_update {
                        info!("Yeni güncelleme tespit edildi: {}", info.latest_version);
                        let _ = std::process::Command::new("notify-send")
                            .args([
                                "-a",
                                "Corten OTA",
                                "-i",
                                "system-software-update",
                                "Corten Desktop Güncellemesi Mevcut",
                                &format!(
                                    "Yeni sürüm ({}) yayınlandı. Güncellemek için Corten Forge'u açın.",
                                    info.latest_version
                                ),
                            ])
                            .spawn();
                    }
                }
                // Sleep for 4 hours
                tokio::time::sleep(Duration::from_secs(4 * 3600)).await;
            }
        }
        "version" | "--version" | "-v" => {
            println!("corten-update v{}", CURRENT_VERSION);
        }
        _ => {
            println!("Corten Desktop OTA Güncelleyici (corten-update)");
            println!("Kullanım: corten-update [KOMUT]\n");
            println!("Komutlar:");
            println!("  check     En son sürümü ve değişiklikleri kontrol eder (varsayılan)");
            println!("  apply     Yeni güncellemeyi indirir ve sisteme kurar");
            println!("  daemon    Arka planda sessizce güncellemeleri tarar");
            println!("  version   Sürüm numarasını gösterir");
        }
    }

    Ok(())
}

