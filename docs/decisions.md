# Corten Mimari Karar Belgesi (ADR)

Bu belge, **Corten Masaüstü Ortamı** projesinde alınan temel mimari, teknik ve tasarım kararlarını ve gerekçelerini kayıt altına alır.

---

## 1. Lisans Stratejisi
- **Karar:** 
  - **Kütüphaneler (`corten-theme`, `corten-config`, `corten-ipc`):** `MIT` lisansı.
  - **Son Kullanıcı Uygulamaları (Panel, Başlatıcı, Kilit, Greeter, Compositor, Ayarlar):** `GPL-3.0` lisansı.
- **Gerekçe:** Temel kütüphanelerin ekosistem genelinde (üçüncü taraf araçlar dahil) rahatça kullanılmasını ve benimsenmesini sağlamak; buna karşın masaüstü ortamının kendi nihai bileşenlerini ve compositor'ını açık kaynak güvencesinde (copyleft) tutmak.

---

## 2. Repo ve Sürüm Stratejisi
- **Karar:** Çoklu bağımsız Git repoları (`corten-desktop/<proje>`). 
- **Sürüm Bağımlılığı:** Geliştirmenin erken evrelerinde crates.io yayın yükü ve isim çakışmalarını önlemek için ortak kütüphaneler sabit git etiketleri (`tag` / `rev`) ile bağlanacak. Kararlı `v1.0.0` aşamasında crates.io'ya geçilecek.
- **Meta Repo:** Faz 9'da tüm repoları birleştiren `corten-desktop` meta reposu **Git alt modülleri (submodules)** ve bir "Sürüm Uyumluluk Tablosu" kullanacak.

---

## 3. UI Araç Kiti (GUI Toolkit)
- **Karar:** `iced` + `iced_layershell`.
- **Gerekçe:** 
  - %100 saf Rust ve Elm mimarisi (durum, mesaj, güncelleme, görünüm).
  - Wayland `wlr-layer-shell` entegrasyonu (`iced_layershell`) panel, başlatıcı ve bildirimler için hazır.
  - System76'nın COSMIC masaüstü ile geniş bir açık kaynak referans kodu bulunması.
- **Kritik Kural:** API değişimlerinden etkilenmemek için tüm Corten bileşenlerinde `iced` sürümü kesin olarak sabitlenecek (`Cargo.toml` üzerinde tam sürüm kilitli).

---

## 4. Süreçler Arası İletişim (IPC)
- **Karar:** Standart D-Bus (`zbus`) ana yol; gerektiğinde yüksek hacimli veya düşük gecikmeli veri için Unix Domain Socket (JSON-RPC/IPC).
- **Gerekçe:**
  - Panel, bildirim servisi ve başlatıcı CLI komutlarıyla (örneğin kısayol tuşlarıyla) uzaktan tetiklenebilmelidir.
  - `org.freedesktop.*` standartları D-Bus üzerinde çalıştığı için `zbus` doğal bir tercihtir.
  - Ortak CLI/IPC tetikleme yardımcıları Faz 1'deki `corten-ipc` crate'inde toplanacaktır.

---

## 5. Tasarım Dili ve Kimlik
- **Tema:** **Corten Paleti**
  - Arka Plan: Koyu antrasit / endüstriyel koyu gri (`#18191B` - `#1E2024`)
  - Vurgu Rengi: Sıcak Corten pası / patinalı bakır (`#D96B43` - `#C45A32`)
  - Metin / İkincil Vurgu: Yumuşak krem / kırık beyaz (`#EAE5DE`)
  - Kenarlık & Ayrım: İnce, fırçalanmış metal tonları (`#2D3139`)
- **Tipografi:** 
  - UI Metinleri: **Inter**
  - Sayılar, Saat, Terminal ve Kod: **JetBrains Mono**
- **İkonlar:** **Lucide Icons** (modern, hafif, vektörel SVG uyumlu).

---

## 6. Pano (Clipboard) ve Yardımcı Süreçler
- **Karar:** Başlatıcı içindeki pano geçmişi sıfırdan daemon yazmak yerine `cliphist` gibi kanıtlanmış bir arka plan aracının çıktısını okuyan ince bir kullanıcı arayüzü olacak.
- **Güvenlik Notu:** `cliphist` geçmişi şifresiz tuttuğu için bu durum kullanıcı dokümantasyonunda belirtilecek.
- **Oturum:** Masaüstü oturum başlatma betiğinde `wl-paste --watch cliphist store` arka planda çalıştırılacak.
