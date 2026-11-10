# Masaüstü Ortamı Roadmap'i

> Ad: **Corten** (`corten-desktop`)
> Hedef: Parça parça, her biri kendi başına kullanılabilen Rust araçlarından oluşan, sonunda tek bir bütün olarak birleşen Wayland masaüstü ortamı.

---

## Temel ilkeler

1. **Her parça bağımsız ve bitmiş olmalı.** Tek başına kurulup kullanılabilir, kendi README'si, ekran görüntüsü ve sürümü olur.
2. **Her faz bir "bitti" tanımıyla kapanır.** Tanım karşılanmadan sonraki faza geçilmez.
3. **Ortak temel baştan kurulur.** Tema, yapılandırma biçimi ve kod standartları parçalar arasında aynıdır.
4. **Önce kendi masaüstünde günlük kullan.** Her parçayı Sway/Niri üzerinde kullanarak gerçek hataları yakala.
5. **Her fazın çıktısı görünür olmalı.** README'nin tepesinde GIF/ekran görüntüsü, kısa bir blog/paylaşım.
6. **Kapsam kontrolü.** Yeni fikirler "Backlog" bölümüne yazılır, mevcut fazı büyütmez.

---

## Genel bakış

| Faz | Parça | Çıktı |
|-----|-------|-------|
| 0 | Hazırlık ve temel | Org, ad, kurallar, araç kiti kararı |
| 1 | `theme` + `config` kütüphanesi | Ortak tasarım jetonları ve yapılandırma |
| 2 | Bildirim sunucusu | `org.freedesktop.Notifications` uygulaması |
| 3 | Uygulama başlatıcı | Bulanık aramalı layer-shell başlatıcı |
| 4 | Panel / durum çubuğu | Saat, çalışma alanları, ses, ağ, pil, tepsi |
| 5 | Kilit ekranı | `ext-session-lock` tabanlı kilit |
| 6 | Oturum açma ekranı | `greetd` greeter'ı |
| 7 | Compositor | Smithay tabanlı, tek fikirli compositor |
| 8 | Ayarlar uygulaması | Tüm parçaları yapılandıran arayüz |
| 9 | Birleştirme ve yayın | Meta repo, paketleme, dokümantasyon, tanıtım |
| 10 | İleri Mühendislik & GPU Scissor | Sıfır taşma, donanımsal pencere kırpma |
| 11 | Kesirli Ölçekleme & Retina Netliği | wp_fractional_scale_v1 ve wp_viewport entegrasyonu |
| 12 | Frame Senkronizasyonu & Akıcılık | Explicit Sync (linux-drm-syncobj-v1) & wp_presentation_time |
| 13 | Popover & Bağlam Menü Mimarisi | Iced sınırlarını aşan katmanlı overlay sistemi |
| 14 | Oturum Dayanıklılığı & Çökme Koruması | corten-session socket handover & crash recovery |

---

## Faz 0: Hazırlık ve temel

**Amaç:** Sonradan değiştirmesi pahalı kararları baştan vermek.

- [x] **Proje adı seçildi: Corten** (GitHub org/meta repo: `corten-desktop`)
- [x] **Repo stratejisi:** Bağımsız Git repoları. Sürüm kaymasını önlemek için erken aşamada crates.io yerine sabit etiketli (`tag` / `rev`) git bağımlılığı kullan; stabilite oturunca crates.io'ya geç.
- [x] **IPC mimarisi kararı:** Standart D-Bus (`zbus`) ve gerektiğinde Unix Domain Socket (JSON-RPC/IPC) kullanımı.
- [x] **Ortak kod standartları:** `rustfmt.toml` (100 sütun, temiz formatlama), katı `clippy` (`pedantic` uyarıları -D warnings ile CI'da hata), commit standartları ([docs/commits.md](file:///home/shura/Projects/WaylandDesktopProject/docs/commits.md))
- [x] **Repo şablonu ve CI hazırlandı:** GitHub Actions CI iş akışı ([.github/workflows/ci.yml](file:///home/shura/Projects/WaylandDesktopProject/.github/workflows/ci.yml) fmt + clippy + test), lisans ayrımı netleştirildi
- [x] **UI araç kiti kararı: `iced` seçildi** (+ `iced_layershell`). Gerekçe için aşağıdaki "Araç kiti kararı: iced" bölümüne bak.
- [x] **Tasarım yönü kararlaştırıldı:** Corten Paleti (antrasit arka plan, sıcak bakır/pas vurgusu, krem metin), Inter + JetBrains Mono yazı tipleri, Lucide ikon seti. Tasarım taslağı Faz 1'deki `corten-theme` önizleme uygulamasında doğrudan canlı kodla icra edilecek.
- [x] **Geliştirme günlüğü başlatıldı:** [DEVLOG.md](file:///home/shura/Projects/WaylandDesktopProject/DEVLOG.md) ilk girdiyle oluşturuldu.

**Bitti tanımı:** CI şablonu, standartlar ve karar belgesi ([docs/decisions.md](file:///home/shura/Projects/WaylandDesktopProject/docs/decisions.md)) hazır. **Faz 0 TAMAMLANDI!**

### Araç kiti kararı: iced

**Neden iced:**
- Saf Rust, tek dilde kalınır. Elm tarzı mimari (durum, mesaj, güncelleme, görünüm).
- Layer-shell için `iced_layershell` mevcut; panel, başlatıcı ve bildirim gibi parçalar için uygun.
- COSMIC masaüstü iced kullanıyor, yani büyük bir referans proje var.
- Tema sistemi esnek; tüm parçalar `theme` crate'inden beslenebilir.

**Değerlendirilen alternatifler:**
- **Slint:** Görsel ve animasyon tarafı çok güçlü, canlı önizleme araçları var. Layer-shell için ek çözüm gerekebilir, ayrıca yeni bir dil (`.slint`) ve lisans koşulları var. Cilalı görünüm öncelik olursa güçlü ikinci seçenek.
- **GTK4 (`gtk4-rs` + `gtk4-layer-shell`):** Olgun, belgeleri bol, erişilebilirlik ve giriş yöntemleri çok iyi. Rust bağlayıcıları hantal, GTK bağımlılığı getiriyor.

**iced ile çalışırken dikkat edilecekler:**
- **Sürümü sabitle.** iced'in API'si sürümler arasında epey değişiyor. `Cargo.toml`'da tam sürüm numarası kullan ve tüm parçalarda aynı sürümde kal. Güncellemeyi bilinçli ve toplu yap.
- **Layer-shell köprüsünü erken doğrula.** `iced_layershell`'in seçilen iced sürümüyle uyumunu ilk iş kontrol et. İlk deneme: ekranın köşesinde, kenarlıksız, tema renklerinde bir kutu açmak.
- **Referans olarak libcosmic'e bak.** System76'nın iced üzerine kurduğu bileşen kütüphanesi. Kopyalamadan, bileşenleri nasıl organize ettiklerini ve temayı nasıl kurduklarını oku.
- **Erişilebilirlik zayıf nokta.** iced'de ekran okuyucu desteği sınırlı. README'de "bilinen sınırlama" olarak dürüstçe belirt; derinleştirme Backlog'da.

---

## Faz 1: Temel kütüphaneler repo'su (`theme`, `config`, `ipc`) - [TAMAMLANDI]

**Amaç:** Tüm parçaların paylaşacağı görünüm, yapılandırma ve IPC yardımcılarını içeren temel repo (bağımlılıkları hafif tutmak için tek repoda 3 ayrı crate'li Cargo workspace).

**Kapsam (3 ayrı crate):**
- [x] **`theme` crate'i:** Corten koyu/açık renk jetonları, 12px köşe yuvarlaklığı (`Radius::MD`), 4px-48px `Spacing`, `Typography`, gömülü `lucide.ttf` ikon fontu, `iced` widget stilleri, WCAG AA (4.5:1) matematiksel kontrast birim testi.
- [x] **`config` crate'i:** `~/.config/corten/` XDG standardı, TOML serileştirme, eksikse otomatik varsayılan oluşturma, `notify` ile canlı arka plan izleme (`ConfigManager<T>`).
- [x] **`ipc` crate'i:** `org.corten.*` D-Bus arayüz proxy'leri (`LauncherControl`, `NotificationControl`, `PanelControl`), oturum bus bağlantısı ve sunucu yardımcıları.
- [x] **`theme-preview`:** Tasarım jetonlarını, panel simülasyonunu, açık/koyu mod geçişini ve buton durumlarını ekranda canlı test eden iced uygulaması.

**Bitti tanımı:**
- Tek repo içinde 3 bağımsız crate (`corten-theme`, `corten-config`, `corten-ipc`)
- Bütün testler ve clippy yeşil (`cargo test`, `cargo clippy` sıfır uyarı)
- Her crate için dokümante edilmiş README dosyaları
- Ekranda canlı doğrulanmış ve glitch'ten arındırılmış `theme-preview` uygulaması. **Faz 1 TAMAMLANDI!**

---

## Faz 2: Bildirim sunucusu (`corten-notify`) - [TAMAMLANDI v0.1.0]

**Amaç:** İlk görünür, bitirilebilir, bağımsız ürün.

**Kapsam:**
- [x] `org.freedesktop.Notifications` D-Bus arayüzü (`zbus` ile): `Notify`, `CloseNotification`, `GetCapabilities`, `GetServerInformation`
- [x] Layer-shell ile ekranın köşesinde bildirim kartları (wlr-layer-shell)
- [x] Acil durum seviyeleri (Low, Normal, Critical), zaman aşımı sayacı, kapatma butonu
- [x] Sıfır ekran blokajı: 1x1 piksel dinamik katman boyutlandırma ile bildirim yokken ekranda sıfır alan kaplama ve tam şeffaflık
- [x] Dahili `--test` modu ile mevcut masaüstünü (KDE vb.) bozmadan test edebilme
- [x] `corten-theme` ve gömülü Lucide ikonlarını kullanır
- [x] `corten-config` ile dinamik konumlandırma (varsayılan Sağ Alt, değiştirilebilir)
- [ ] *v0.2.0:* Giriş/çıkış geçiş animasyonları (fade-in / slide-in) ve bildirim geçmişi modalı

**Bitti tanımı:**
- Sway/Niri/Hyprland ve Wayland üzerinde hatasız çalışır
- Sıfır ekran blokajı ve test modu doğrulandı
- Dokümante edilmiş README ve `v0.1.0` hazır. **Faz 2 TAMAMLANDI!**

---

## Faz 3: Uygulama başlatıcı

**Kapsam:**
- [x] `.desktop` dosyalarını XDG kurallarına göre bulma ve ayrıştırma (`Exec` alan kodları, `NoDisplay`, yerelleştirme)
- [x] Bulanık arama (`nucleo` crate'i)
- [x] Klavye odaklı Spotlight arayüzü (`↑`/`↓` ile senkron otomatik kaydırma, `Tab` ile mod geçişi, `↵` çalıştırma, `Esc` kapatma)
- [x] D-Bus `org.corten.Launcher` kontrolü (`Toggle`, `Open`, `Close`)
- [x] **Pano geçmişi modu (Clipboard - cliphist entegrasyonu):** `cliphist list` okuma ve `cliphist decode | wl-copy` ile geri kopyalama, eksik bağımlılık için kullanıcı dostu uyarı
- [x] Ultra-minimalist 4px ince kaydırma çubuğu ve sıfır gecikmeli senkron durum yönetimi
- [x] Canlı yapılandırma desteği (`corten-config` / `~/.config/corten/launcher.toml`)
- [ ] *v0.2.0:* Hesap makinesi, emoji arama ve kullanım sıklığı sıralaması eklentileri

**Bitti tanımı:** Günlük başlatıcı olarak Wayland üzerinde test edildi, README hazır, `v0.1.0`. **Faz 3 TAMAMLANDI!**

---

## Faz 4: Panel / durum çubuğu

**Amaç:** Projenin en görünür parçası ve en çok sistem entegrasyonu öğrenilen faz.

- [x] Saat ve takvim (canlı senkronize iki satırlı saat ve tarih)
- [x] Çalışma alanı/pencere listesi (KWin D-Bus, Sway IPC ve `ext-workspace` uyumlu dinamik masaüstü geçişi ve senkronizasyonu)
- [x] Açık/sabitlenen uygulamalar ve aktif durum takibi (`/proc` gerçek zamanlı süreç tespiti ile açık uygulama nokta göstergeleri)
- [x] Ses: PipeWire/WirePlumber üzerinden (ses durumu ve toggle mute)
- [x] Ağ: NetworkManager CLI/D-Bus durum entegrasyonu
- [ ] Pil ve güç: UPower
- [ ] Bluetooth: BlueZ
- [ ] Sistem tepsisi: StatusNotifierItem protokolü
- [ ] Parlaklık, medya denetimi (MPRIS)
- [x] D-Bus/IPC arayüzü (`zbus`): Panel modüllerini komut satırından tetikleme/açıp kapatma (Faz 1'deki ortak IPC kalıbı ile)
- [ ] Hızlı tetikleyiciler: Pano geçmişini açma veya hazır ekran alıntısı aracını (`grim`/`slurp`) çalıştıran butonlar

**Bitti tanımı:**
- Modül mimarisi dokümante edilmiş (üçüncü kişi modül ekleyebilir)
- Çoklu monitör ve HiDPI testleri
- `v0.1.0`, ekran görüntüleri ve GIF

**Risk:** Kapsam büyümeye en yatkın faz. Önce saat, çalışma alanı, ses, pil ve temel D-Bus arayüzü ile bitir; bluetooth, pano/ekran alıntısı tetikleyicileri ve gelişmiş modülleri `v0.2.0`'a bırak.

---

## Faz 5: Kilit ekranı

- [x] Gerçek `ext-session-lock-v1` kilit protokolü ve compositor tarafı yüzey yönetimi
- [x] PAM ile sistem kimlik doğrulama (`pam` crate ve `login` stack entegrasyonu)
- [x] Sistem duvar kağıdı tespiti ve Gaussian blur bulanıklık önbellekleme
- [x] Katmanlı akrilik buzlu cam kilit kartı (frosted glass card overlay)
- [x] Büyük 76px saat ve tarih göstergesi
- [x] Kullanıcı avatarı (`Icon::LOCK`) ve dinamik kullanıcı adı (`users` crate)
- [x] Güvenli parola giriş kutusu (`text_input`, `secure(true)`) ve `↵` onayı
- [x] Başarısız deneme geri bildirimi (hata durumu, kırmızı kenarlık ve uyarı mesajı)
- [x] Greeter ile ortak güvenli credential lifecycle (`corten-auth`); deneme sonrası secret temizleme

**Bitti tanımı:** `corten-lock` akrilik cam ve bulanık duvar kağıdı desteğiyle test edildi, derlendi ve `v0.1.0` tamamlandı. **Faz 5 TAMAMLANDI!**

---

## Faz 6: Oturum açma ekranı

- [x] `greetd` IPC protokolü entegrasyonu (`greetd_ipc` sync-codec)
- [x] Kullanıcı adı ve güvenli parola girişi (`text_input`, `secure(true)`)
- [x] Oturum tespiti ve seçimi (`/usr/share/wayland-sessions` ve `xsessions` dinamik arama)
- [x] Sağ üstte sistem güç menüsü (Kapat, Yeniden Başlat, Uyku - `systemctl`/`loginctl`)
- [x] Tema ve tasarım: Bulanık duvar kağıdı, akrilik buzlu cam kartı, büyük dijital saat
- [x] Geliştirme/test modu (GREETD_SOCK tanımlı olmadığında güvenli masaüstü önizleme)
- [x] Lock ile ortak credential lifecycle ve backend'den bağımsız auth state yönetimi

**Bitti tanımı:** `corten-greeter` ilk sürüm hazır, derlendi, tüm testlerden geçti. **Faz 6 TAMAMLANDI!**

---

## Faz 7: Compositor

**Amaç:** Projenin taç mücevheri. Bu noktada elinde zaten test edilmiş bir araç ekosistemi olduğu için compositor'ı yazarken gerçek uygulamalarla çalışırsın.

**Önemli:**
- Risk dağıtmak için Faz 4 sırasında bir **prototip** yap (Smithay ile pencere açan minimal compositor). Gerçek faz başladığında temel taşları zaten bilirsin.
- **"Tek fikir"i erken seç:** Mimariyi doğrudan etkilediği için scrolling tiling (Niri tarzı), dinamik tiling (bspwm/Hyprland), güçlü animasyonlar veya kendi panel/başlatıcınla derin entegrasyondan hangisi olacağına Faz 4 prototipinde karar ver.

**Aşamalar:**

1. [x] **İskelet & İç İçe Ortam (Winit):** Smithay ile winit backend, xdg-shell, pencere açma, odaklanma ve yeniden boyutlandırma
2. [x] **"Tek fikir" (Dinamik Master-Stack Tiling):** 12px boşluklu (gap), sol ana ve sağ dikey yığın düzenli `LayoutManager`, `Super+H`/`L` dinamik oran ayarı
3. [x] **Klavye & Fare Yönetimi:** `Super+Return` başlatıcı, `Super+Q` pencere kapatma, `Super+J/K` odak geçişi, fare ile tıklanan pencereyi odaklama
4. [x] **Protokol uyumu (`wlr-layer-shell` & Popups):** `corten-panel`, `corten-launcher`, `corten-notify` entegrasyonu, `non_exclusive_zone` ile panel boşluğu koruma ve popup yönetimi
5. [~] **Gerçek donanım:** DRM/KMS + libinput + libseat (TTY'de bağımsız çalışma; kod yolu hazır, donanım smoke testi bekliyor)
6. [ ] **Portal ve yakalama:** `wlr-screencopy` / `ext-image-copy-capture` ve PipeWire ekran paylaşımı

**Bitti tanımı:**
- [x] Smithay nested pencere içinde XDG uygulamaları ve Corten layer-shell araçları birlikte çalışır
- [x] Dinamik tiling, klavye kısayolları ve fare odağı tam fonksiyonel
- [x] Clippy ve birim testleri sıfır hata ile geçiyor (`v0.1.0`)

**Risk:** En büyük ve en yarım kalma ihtimali olan parça. Her aşama sonunda çalışan bir sürüm etiketle.

---

## Faz 8: Ayarlar uygulaması - [TAMAMLANDI v0.1.0]

**Kapsam:**
- [x] Tema ve Görünüm: Açık/Koyu mod seçimi, Corten Rust aksan renkleri, duvar kağıdı yolu
- [x] Panel & Dock: Konumlandırma (Top/Bottom), stil (Floating, Islands, Flush), modül görünürlük anahtarları
- [x] Başlatıcı: Spotlight modal genişliği (600px/750px), pano geçmişi açıp kapama
- [x] Bildirimler: Zaman aşımı süresi (3s, 5s, 8s) ve kart genişlikleri
- [x] Compositor: Dinamik tiling boşluk payı (`gap`) ayarı (0px, 12px, 20px)
- [x] Canlı Senkronizasyon: `~/.config/corten/*.toml` dosyalarını görsel arayüzle okuma ve kaydetme
- [x] Merkezi compositor keybinding sözleşmesi: kısayollar `compositor.toml` üzerinden reload edilir ve Settings’te gösterilir

**Bitti tanımı:** Yapılandırma dosyasına hiç dokunmadan masaüstünün temel ayarlarını düzenleyen `corten-settings` tamamlandı. **Faz 8 TAMAMLANDI!**

---

## Faz 9: Birleştirme ve yayın - [v0.1.0 ORKESTRASYON TAMAMLANDI]

**Amaç:** Parçaları tek bir ürün gibi sunmak. CV'de en çok fark yaratan faz.

**Teknik:**
- [x] **Birleştirilmiş Oturum Yöneticisi (`corten-session`):** Tek bir komutla compositor, panel, bildirim sunucusu ve arka plan servislerini senkronize başlatan süreç denetleyicisi
- [x] **Wayland Oturum Tanımı:** `assets/corten.desktop` Wayland oturum dosyası hazırlandı
- [x] **Klavye Kısayol Entegrasyonu:** `Super+Return` (Terminal), `Super+D/Space` (Spotlight Launcher), `Super+I` (Ayarlar), `Super+Q` (Kapatma), `Super+J/K` (Pencere odağı)
- [ ] **Paketleme:** AUR paketleri (`corten-desktop` PKGBUILD) ve GitHub Releases derleme betiği
- [ ] **Uçtan uca testler:** Temiz bir VM'de kurulumdan masaüstüne tam akış

**Sunum:**
- [ ] Ana README: mimari diyagramı, hangi parçanın ne yaptığı, ekran görüntüleri, 60 saniyelik demo videosu
- [ ] `docs/architecture.md`: "neden bu tasarım kararlarını aldım" yazısı (mülakatlarda konuşma malzemesi)
- [ ] Blog yazısı / seri: "Sıfırdan Wayland masaüstü ortamı yazdım" (her faz için kısa yazılar zaten günlükte hazır)
- [ ] Paylaşım: r/unixporn, r/rust, Hacker News "Show HN", Lobsters, Rust topluluk forumları
- [ ] GitHub profili: ilgili repoları pinle, organizasyon profil README'sini düzenle
- [ ] CV satırı: tek bir güçlü cümle + repo bağlantısı + tek bir ölçülebilir sonuç (ör. "N yıldız, M modül, X ms açılış süresi")

**Bitti tanımı:** Birisi bir komutla kurup tam çalışan masaüstüne ulaşabiliyor ve ana README'yi okuyan 2 dakikada projeyi anlıyor.

---

## Faz 10: Ekranda Sıfır Taşma & Donanımsal GPU Scissor (Clipping) - [KOD TAMAM, CANLI DOĞRULAMA BEKLİYOR]

**Amaç:** İstemci uygulamaların (Wayland client) yeniden boyutlandırma anında pencere sınırlarından taşmasını donanım seviyesinde sıfırlamak.

- [x] `corten-compositor`: istemci yüzeyleri `clip_to_body()` ile `CropRenderElement` kullanılarak pencere gövdesi `(x, y + titlebar_h, w, h - titlebar_h)` sınırlarına kırpıldı (udev + winit yolları)
- [x] Fullscreen modunda kırpma atlanır (direct scanout korunur)
- [x] Geriye dönük uyumluluk: 11 compositor testi geçti, clippy'de yeni uyarı yok
- [x] Performans: yeni heap tahsisi yok (iterator + mevcut Vec yeniden kullanımı)

---

## Faz 11: Kristal Netlik & Kesirli Ölçekleme (wp_fractional_scale_v1)

**Amaç:** 1.25x ve 1.5x HiDPI ekranlarda bulanıklığı yok etmek.

- [x] `wp_fractional_scale_v1`, `wp_viewporter`, `wp_presentation` protokolleri zaten kayıtlı ve çalışıyor (denetlendi)
- [x] Hata düzeltildi: commit sırasında tüm yüzeylere birincil çıkışın ölçeği veriliyordu; ikincil monitördeki pencere/layer yüzeyleri artık kendi çıkışının ölçeğini alıyor (`scale_for_surface`)
- [x] Layer-shell yüzeyleri (panel, launcher, bildirim) ölçek değişiminde `refresh_all_surface_scales` ile güncelleniyor
- [ ] Canlı doğrulama: 1.25x / 1.5x çıkışta metin netliği ve karışık DPI monitör geçişi
- [x] Geriye dönük uyumluluk: tüm testler geçti, clippy'de yeni uyarı yok

---

## Faz 12: Frame Senkronizasyonu & Akıcılık (Explicit Sync & Pacing)

**Amaç:** macOS Metal / Windows DWM akıcılığında yırtılmasız ve mikrotitreşimsiz kompozitör döngüsü.

- [x] `linux-drm-syncobj-v1` (Explicit Sync) global'i DRM backend'de açıldı; yalnızca `syncobj_eventfd` destekleniyorsa etkinleşir, aksi halde sessizce devre dışı kalır
- [x] Commit öncesi hook: istemci GPU işi bitmeden (acquire point veya dmabuf implicit fence) yüzey durumu uygulanmaz (`new_surface` pre-commit blocker)
- [ ] `wp_presentation_time` geri bildirim doğruluğunun canlı ölçümü (frame pacing)
- [ ] Canlı doğrulama: AMD/Intel/NVIDIA ve XWayland, yırtılma/titreme testi
- [x] Geriye dönük uyumluluk: testler geçti, clippy'de yeni uyarı yok; winit (iç içe) modunda hook devre dışı, davranış değişmedi

---

## Faz 13: Gelişmiş Popover & Çok Katmanlı Menü Mimarisi - [KOD TAMAM, CANLI DOĞRULAMA BEKLİYOR]

**Amaç:** Iced'in tek pencere kısıtını aşan Wayland katmanlı mikro-overlay menü standardı.

- [x] `corten-panel`: Dock context menu için `Escape` tuşu dinlemesi ve dışarı tıklamayla kapanma (`CloseDockContextMenu`) bağlandı
- [x] `corten-files`: Dosya ve klasörler için sağ tık akrilik bağlam menüsü (`mouse_area.on_right_press`) eklendi (Aç, Yeni Sekmede Aç, Kopyala, Kes, Yeniden Adlandır, Çöpe Gönder, Arşiv Çıkar, Terminalde Aç, AI İncele)
- [x] `corten-files`: `stack!` ve dış şeffaf dismiss overlay'i ile tıklama izolasyonu ve `Escape` tuşuyla anında kapanma sağlandı
- [x] Geriye dönük uyumluluk: Tüm 79 birim testi firesiz geçti, clippy sıfır uyarı verdi

---

## Faz 14: Oturum Dayanıklılığı & Çökme Koruması (Crash Resilience) - [KOD TAMAM, CANLI DOĞRULAMA BEKLİYOR]

**Amaç:** Kompozitör çökse dahi açık uygulamaların ve masaüstü oturumunun çökmeden otomatik toparlanması (auto-recovery).

- [x] `corten-session`: Kompozitör için aktif çökme denetimi (crash supervision) ve maksimum 3 kez otomatik yeniden başlatma (`compositor_restart_count`) eklendi
- [x] Kompozitör çöktüğünde (`!status.success()`) oturum anında TTY'ye düşmek yerine kompozitörü yeniden canlandırır, soketi bekler ve panelleri/uygulamaları yeniden ayağa kaldırır
- [x] Kullanıcının kasıtlı çıkışı (`status.success()` veya SIGTERM/SIGINT) ile istisnai çökmeler birbirinden ayrıldı
- [x] Geriye dönük uyumluluk: Tüm birim testleri firesiz geçti, clippy sıfır uyarı verdi

---

## Faz 15: StatusNotifierItem (SNI) & Standart Sistem Tepsisi (System Tray) - [KOD TAMAM, CANLI DOĞRULAMA BEKLİYOR]

**Amaç:** Harici masaüstü uygulamalarının (Steam, Discord, OBS, Dropbox, NetworkManager) panel dock'una simge bırakabilmesi ve sağ tık menülerini açabilmesi.

- [x] `corten-panel`: `org.kde.StatusNotifierWatcher` D-Bus arayüzü tescili yapıldı (`/StatusNotifierWatcher`)
- [x] `RegisterStatusNotifierItem` tescil döngüsü ile gelen `StatusNotifierItem` servislerinden simge ve durum bilgileri asenkron toplandı
- [x] D-Bus `NameOwnerChanged` dinleyicisi ile uygulamalar kapandığında tepsi ikonunun otomatik temizlenmesi sağlandı
- [x] Sol tık ile `org.kde.StatusNotifierItem.Activate`, sağ tık (`mouse_area`) ile `org.kde.StatusNotifierItem.ContextMenu` çağrısı bağlandı
- [x] Geriye dönük uyumluluk: Tüm 81 birim testi firesiz geçti, clippy sıfır uyarı verdi

---

## Faz 16: Düşük Gecikmeli Oyun & Asenkron Yırtılma (wp_tearing_control_v1) - [KOD TAMAM, CANLI DOĞRULAMA BEKLİYOR]

**Amaç:** Rekabetçi oyunlar ve tam ekran 3D uygulamalar için VSync gecikmesini sıfırlamak.

- [x] `wayland-protocols`: `staging` özelliği etkinleştirilerek `wp_tearing_control_manager_v1` ve `wp_tearing_control_v1` protokolleri derlemeye alındı
- [x] `corten-compositor/src/state.rs`: `is_tearing_allowed_for_output()` kontrolü ile yalnızca tam ekrandaki ve `Async` talep eden istemcilere izin verildi
- [x] `corten-compositor/src/udev.rs`: DRM render pacing döngüsünde tearing aktifken VBlank bloklaması atlandı, anında frame callback ile 200+ FPS sıfır gecikme sağlandı
- [x] Tearing istemeyen normal masaüstü pencerelerinde kilitli VBlank ve yırtılmasız pürüzsüz akış korundu
- [x] Geriye dönük uyumluluk: 13 kompozitör testi ve tüm 83 workspace testi firesiz geçti

---

## Faz 17: İşaretçi Kısıtlamaları & Bağıl Fare Protokolleri - [PLANLANDI]

**Amaç:** FPS oyunları, Blender ve sanal makineler için donanımsal fare kilitleme ve serbest 360 kamera kontrolü.

- [ ] `zwp_pointer_constraints_v1` (Lock & Confine Pointer) Smithay entegrasyonu
- [ ] `zwp_relative_pointer_manager_v1` ile mutlak ekran koordinatları yerine ham `dx/dy` libinput hareketlerini iletme
- [ ] Fare kilitlendiğinde donanımsal imleci gizleme; Alt+Tab veya Super ile kilidi anında çözme

---

## Faz 18: Akıcı Dokunmatik & 1:1 Touchpad Jestleri (Swipe & Pinch) - [PLANLANDI]

**Amaç:** Touchpad üzerinde macOS/GNOME akıcılığında parmak hızına 1:1 kilitli çalışma alanı kaydırma ve Exposé geçişi.

- [ ] `zwp_pointer_gestures_v1` swipe & pinch protokolü entegrasyonu
- [ ] 3 parmak yatay kaydırmada çalışma alanlarının pencerelerle birlikte parmak hızında 1:1 akıcı kayması
- [ ] Jest tamamlandığında sönümlü yay (spring physics) ile çalışma alanına oturma (snap)

---

## Faz 19: Dinamik Eklenti, Scripting & Anlık IPC Olay Akışı Motoru - [PLANLANDI]

**Amaç:** Yeniden derleme gerektirmeden kullanıcıların paneli genişletebilmesi, özel widget ve otomasyon scriptleri yazabilmesi.

- [ ] `corten-ipc`: JSON-RPC tabanlı Unix Domain Socket anlık olay yayıncısı (`window:created`, `workspace:changed`, `audio:volume`)
- [ ] CLI ve betikler için `cortenctl subscribe` ve `cortenctl action` altyapısı
- [ ] Panel için harici betik çıktısı veren modüler `ScriptWidget` desteği

---

## Her faz için tekrarlanan kontrol listesi

- [ ] README: ne yapar, GIF/ekran görüntüsü, kurulum, yapılandırma, bilinen sorunlar
- [ ] Testler ve CI yeşil (`fmt`, `clippy`, `test`)
- [ ] Sürüm etiketi ve `CHANGELOG.md`
- [ ] Geliştirme günlüğüne yazı
- [ ] Günlük kullanımda kendin kullan
- [ ] Bir geri bildirim turu (arkadaş, Discord/Reddit)

---

## Risk yönetimi

| Risk | Önlem |
|------|-------|
| Compositor fazında motivasyon düşmesi | Faz 4 sırasında prototip yap; her aşamada çalışan sürüm etiketle |
| Kapsam büyümesi | Backlog kuralı; her fazda "v0.1 = minimum" |
| Tasarım tutarsızlığı | Tüm bileşenler yalnızca `theme` crate'inden değer alır |
| Parçalar arası uyumsuzluk | Ortak yapılandırma şeması ve sürüm uyumluluk tablosu |
| Gecikme | Süre baskısı yok; gerekirse son fazlardaki "isteğe bağlı" modülleri `v0.2`'ye ertele |
| Tükenmişlik | Fazlar arasında kısa molalar, küçük ara hedefler |

---

## Backlog (Kategori 3: Olsa da Olur / Gelecek Hedefleri)

- [x] **Özel Saf Rust Dosya Yöneticisi (`corten-files`):** Corten antrasit ve pas temalı, hafif ve modern çift panelli dosya gezgini.
- [x] **Pencere Genel Bakışı (Window Overview / Exposé - `Super+W`):** Smithay üzerinde tüm pencereleri ekrana simetrik döşeyen genel bakış modu, tıklamayla pencereye odaklanma.
- [x] **Gelişmiş Çoklu Dokunmatik Hareketler (Touchpad Gestures):** 3 parmakla yatay kaydırma ile workspace geçişi, 3 parmak yukarı kaydırma ile Exposé/Overview, 4 parmak aşağı kaydırma ile masaüstünü göster.
- [x] **Hava Durumu & Takvim Popover'ı (`corten-panel`):** Panel takviminde canlı hava durumu (sıcaklık, durum, nem, rüzgar, 3 günlük tahmin) ve interaktif ay takvimi.
- [x] **Corten Güç & Oturum Merkezi (`Super+X` / `corten-panel`):** Akrilik modal üzerinde 5 ana aksiyon kartı (Kilitle, Askıya Al, Oturumu Kapat, Yeniden Başlat, Kapat) ve onay koruması.
- [x] **Gelişmiş Denetim Merkezi (Control Center - `corten-panel`):** Ses ve parlaklık canlı kaydırıcıları, ses çıkış aygıtı seçimi, Wi-Fi tara/bağlan, gece ışığı, kafein ve 7 dilli yerelleştirme.
- [x] **Masaüstü Quick Look & Dev Project Hızlı Aksiyonları (`corten-files` & `corten-launcher`):** Space tuşuyla anlık dosya önizlemesi (Quick Look), `:p` modunda git branch ve dirty dosya sayısı ile doğrudan IDE/Terminal/Dosyalar butonları.
- [x] **Kilit Ekranı MPRIS Medya Kontrolleri & Hata Titreşimi (`corten-lock`):** Kilit ekranından ayrılmadan müzik çaları yönetme (oynat/duraklat/önceki/sonraki, parça/sanatçı bilgisi) ve hatalı parolada pürüzsüz yatay sarsılma (shake) animasyonu.
- [x] **Canlı Çalışma Alanı Uygulama Rozetleri & Güç Profili Seçici (`corten-panel`):** Panelde çalışma alanı numaralarının yanında o masaüstünde açık olan uygulamaların canlı mikro simgeleri (`>_`, `🌐`, `💻`, `🎵`, `💬`, `📁`) ve Denetim Merkezinde 3 kademeli UPower güç profili seçimi (Tasarruf, Dengeli, Performans).
- [x] **Entegre Terminal Çekmecesi & Başlatıcı Renk Dönüştürücü (`corten-files` & `corten-launcher`):** Dosya yöneticisinde `F4` / ``Ctrl+` `` ile doğrudan dizin içinde komut koşan gömülü terminal çekmecesi, başlatıcıda boşken sık kullanılanlar çubuğu ve anlık HEX/RGB/HSL renk dönüştürücü.
- [x] **Ekrana İğneleme & Yapay Zeka Kompozitör Yönetimi (`corten-snipper` & `corten-ai`):** Ekran kesitini masaüstünün üzerinde her zaman üstte duran akrilik karta dönüştüren `--pin` modu ve AI asistan üzerinden doğal dille pencere/çalışma alanı kontrolü (*"masaüstü 2'ye geç"*, *"pencereyi sola yasla"*, *"masaüstünü göster"*).
- [x] **Grup Bildirim Rozetleri & Uygulama Bazlı Toplu Kapatma (`corten-notify`):** Aynı uygulamadan gelen bildirimlerde akrilik `+N` rozeti, tek tıkla uygulamaya ait tüm bildirimleri kapatma ve bildirim merkezi geçmişinde toplam sayaç hapı.
- [x] **Dinamik Dock, Çoklu Pencere Noktaları, İndirilenler Yığını ve Çöp Kutusu Entegrasyonu (`corten-panel`):** Çalışan pencerelerin dock'a anlık yansıması, pencere sayısını gösteren akrilik durum noktaları (`●`), tıklamayla pencereye anında odaklanma (`focus_window`), son indirilen dosyaları önizleyen interaktif `📥 İndirilenler Yığını (Downloads Stack)` kartı ve `🗑️ Çöp Kutusu` sayaç rozeti ile hızlı temizleme.
- [x] **Dock Sağ Tık Menüsü & Panel Mini Görev Yöneticisi (`corten-panel`):** Dock üzerindeki uygulamalara sağ tıklandığında açılan akrilik bağlam menüsü (*Yeni Pencere Aç*, *Öne Getir*, *Kapat/Sonlandır*); panelde canlı donanım bellek hapı (`📊 %RAM`) ve tıklandığında en çok bellek tüketen süreçleri listeleyen, tek tıkla `✕ Sonlandır` aksiyonu sunan ve `corten-monitor`e bağlanan Mini Görev Yöneticisi kartı.
- [x] **Gelişmiş Çoklu Sekme ve Dizin Çoğaltma (`corten-files`):** Dosya yöneticisinde seçili klasörü doğrudan `⧉ Yeni Sekmede Aç` aksiyonu, sekme çoğaltma (`⧉ Duplicate`), diğer sekmeleri kapatma ve `Ctrl+Shift+Tab` ile akıcı sekme geçişi.
- [x] **Hassas Bilgi Gizleme & Geri Sayımlı Ekran Alıntısı (`corten-snipper`):** Ekran kırpma aracında 3s/5s zamanlayıcı ve geri sayım kartı (`--delay` parametresi ve arayüz butonları), yakalanan/iğnelenen ekran görüntüsü üzerinde hassas verileri (şifre, kimlik, API key) anında karartan `🔒 Hassas Bilgi Gizle (Redact)` fırçası, geri alma (`Ctrl+Z`) ve `💾 Kaydet` ile sansürlü PNG çıktısı alma.
- [x] **Canlı Pencere Önizleme Kartları, Now Playing Dinamik Adası, Akıllı Gizlenme ve Ağ Trafiği Monitörü (`corten-panel`):** Birden fazla pencere açıldığında dock simgesine tıklandığında beliren interaktif pencere seçici kartları (*Window Previews*); medya çalarken beliren animasyonlu mini oynatıcı kapsülü (*ılılı Dinamik Ada*); pencereler yaklaştığında otomatik saklanan akıllı kaçınma (*Intellihide / Dodge*); toolbar üzerinde canlı indirme/yükleme hızı monitörü (*↓ MB/s ↑ KB/s*) ve kalıcı yapılandırma destekli *Dock'a Sabitle / Kaldır* sistemi.
- [x] **Işık Yakalayan Akrilik Cam & Speküler Kenarlık (`acrylic_glass_style` in `corten-theme`):** Üst kenardan ışık alan mikro speküler vurgu, 24px derin ortam gölgesi ve yarı saydam buzlu koyu arka plan ile Denetim Merkezi, Mini Görev Yöneticisi, Dock menüleri ve popover'larda üst segment cam estetiği.
- [x] **4 Rafine Endüstriyel Renk Paleti (Corten Patina Engine):** WCAG AAA kontrast uyumlu 4 patina seçeneği (`🍂 Corten Pas`, `🌊 Patina Zümrüt`, `⚡ Dövme Titanyum`, `🌋 Akkor Altın`); Denetim Merkezinden anlık çip seçimi ve tüm sisteme (`corten-panel`, `corten-compositor`, `corten-widgets`) otomatik yayılım.
- [x] **Dokunsal Yay Fiziği & Mikro-Etkileşimler (`Transition::spring` in `corten-theme` & `corten-panel`):** Popover, modal ve açılır menülerde doğal elastik oturma ve mikro sıçrama sunan sönümlü yay animasyon eğrisi.
- [x] **Aktif Pencere Atmosferik Parıltı Halkası (Active Window Halo Glow in `corten-compositor`):** Aktif odaktaki pencere çevresinde Smithay GLSL donanım hızlandırmalı yumuşak Gaussian neon halo parıltısı (18px yayılım, 8px köşe eğrisi, 0.72 parlaklık); seçilen Patina tonuna göre anlık senkronizasyon.
- [x] **Masaüstü HUD Bilgi Kartları & Dairesel Halka Göstergeleri (`corten-widgets`):** Masaüstünde doğrudan duvarkağıdı katmanında çalışan akrilik dijital saat, donanım göstergeleri (`RadialGauge` CPU/RAM dairesel halkaları), canlı ses dalga formu visualizer'ı ve Patina paletiyle tam uyumlu tema motoru.
- [x] **Dinamik Duvar Kağıdı Rengi Çıkarıcı & Akıllı Uyum (`corten-wallpaper` & `crates/theme`):** Duvarkağıdı piksellerinden otomatik ambiyans rengi ve en yakın Corten Patina varyantını (`match_from_rgb`) çıkarıp tüm masaüstü kompozitör halo ışığına ve dock temasına anlık uyarlayan Ambiance Sync motoru.
- [x] **Birleşik Dinamik Ada OSD Sistemi (`corten-panel`):** Ses ve parlaklık ayarlarında akıcı sıvı ilerleme çubuğu, mikrofon susturulduğunda beliren koruma kalkanı rozeti (`🛡️ / 🎙️`) ve düşük pil uyarı kapsülü ile pürüzsüz akrilik OSD.
- [x] **Akrilik Başlık Çubuğu & Endüstriyel Durum Rozetleri (`corten-compositor`):** Odaktaki pencerenin üstünde 2px Corten Patina vurgu çizgisi ve sol üstte dövme çelik perçin rozeti (`Corten Rivet Badge`).
- [x] **Masaüstü Sağ Tık Akrilik Tuvali (`corten-compositor` & `corten-panel`):** Boş masaüstüne sağ tıklandığında veya `Super+D` ile açılan, duvarkağıdı slayt gezgini, hızlı araçlar (Snipper, Terminal, Dosyalar, Monitör) ve Zen Modu (HUD gizle/göster) barındıran akrilik istasyon.
- [x] **Canlı PipeWire Stereo VU-Meter (`corten-panel`):** Müzik veya medya akarken toolbar ses butonunun yanında ve Denetim Merkezinde canlı dans eden profesyonel balistik stereo ses genlik çubukları (`VU ▂▄▆█`).
- [x] **Akıllı Pencere Kuralları ve Çalışma Alanı Otomatik Yönlendirme (`corten-compositor`):** Hesap makinesi, diyaloglar ve ayarlar için otomatik yüzen mod; kod, web, sohbet ve medya uygulamalarının ilgili çalışma alanlarına otomatik atanması.
- [x] **StatusNotifierItem (SNI / AppIndicator) Sistem Çekmecesi (`corten-panel` - FAZ 15):** `org.kde.StatusNotifierWatcher` ve `/StatusNotifierItem` D-Bus köprüsü ile Steam, Discord, Spotify simgelerinin dock ve toolbar tepsisinde canlı çalışması, sol tık Activate ve sağ tık ContextMenu entegrasyonu.
- [x] **Düşük Gecikmeli Oyun & Asenkron Yırtılma (`wp_tearing_control_v1` - FAZ 16):** Rekabetçi FPS oyunlarında DRM VBlank gecikmesini bypass eden, karesel yırtılmaya izin vererek sıfır girdi gecikmesi sunan `wp_tearing_control_manager_v1` ve asenkron sayfa yenileme desteği.
- [x] **İşaretçi Kısıtlamaları & Bağıl Hareket (`zwp_pointer_constraints_v1` & `zwp_relative_pointer_manager_v1` - FAZ 17):** 3D modelleme (Blender) ve FPS oyunları için pencereye hapsetme/kilitleme (`Confine` / `Lock`), donanımsal imleç gizleme ve ham libinput `dx/dy` (`RelativeMotionEvent`) akışı.
- [x] **Akıcı Dokunmatik & 1:1 Touchpad Jestleri (`zwp_pointer_gestures_v1` - FAZ 18):** Smithay `PointerGesturesState` entegrasyonu, istemciler için 2 parmaklı swipe & pinch olay akışı (web zoom/sayfa gezinme), sistem için 1:1 yatay pencere öteleme, 3 parmak çalışma alanı geçişi ve 3+ parmak Exposé/Overview pinch tetikleyicisi.
- [x] **Dinamik Eklenti, Scripting & Anlık IPC Olay Akışı (`cortenctl` - FAZ 19):** Unix domain socket üzerinde JSON-RPC olay yayıncısı (`workspace:changed`), resmi `cortenctl` CLI aracı (`subscribe`, `action`, `panel`, `notify`) ve panel için zaman aşımı korumalı `custom_scripts` eklenti motoru.
- [x] **Çok dilli arayüz (`corten-i18n`):** 7 dilli güçlü tip güvenli yerelleştirme motoru (TR, EN, DE, FR, ES, RU, JA) ve dinamik tarih biçimlendirici.
- [x] **Direct Scanout & DRM Plane Bypass (FAZ 20):** Tam ekran oyun ve video oynatıcılarda kompozitör halo/shader döngüsünü tamamen atlayarak istemci DMA-BUF tamponunu doğrudan monitörün donanım tarama katmanına (`PrimaryPlaneElement::Element`) basma ve `update_surface_primary_scanout_output` ile 0ms ek gecikme sağlama.
- [x] **Touchpad Kinetik Momentum & Fling Fiziği (FAZ 21):** Jest bırakıldığında son 150ms'lik mikrosaniyelik delta'lardan px/s hız vektörü hesabı (`calculate_gesture_velocity`), hızlı fırlatma (fling) ile eşik aranmaksızın anında çalışma alanı/Exposé geçişi, `step_kinetic_physics` ile elastik yay sönümlemesi ve 240Hz uyarlamalı kare hızlandırması.
- [x] **Pencere Kenar Manyetizması (Edge Snapping & Magnetism - FAZ 22):** Kayar pencereler taşınırken veya boyutlandırılırken ekran sınırlarına, panel/dock kullanılabilir alanına ve komşu pencerelere 14px mesafede manyetik olarak pürüzsüz yapışma (`snap_window_to_edges`, `snap_window_resize`).
- [x] **Dinamik Ekran Sıcak Takma (Hotplug Re-Layout - FAZ 23):** Harici monitör çıkarıldığında pencerelerin ve kayar geometrilerin güvenle birincil ekrana aktarılması (`drain_all_windows`, `rescue_window`), görünür alan sınırlarına (`clamp_floating_geometries_to_area`) kırpılması, yeni monitör bağlandığında otomatik CRTC tahsisi, OSD bildirimi ve `output:connected`/`output:disconnected` olay yayınları.
- [x] **Derinlemesine Erişilebilirlik & Kapsayıcı Tasarım (a11y & AT-SPI / AccessKit - FAZ 24):** AT-SPI2 / AccessKit D-Bus köprüsü ve ekran okuyucu entegrasyonu, WCAG AAA yüksek karşıtlık modu (>18:1), Brettel/Machado renk matrisi tabanlı renk körlüğü filtreleri (Protanopi, Döteranopi, Tritanopi, Monokrom), dinamik 1.0x-2.5x font ve 48px büyük imleç boyutlandırma, 3px yüksek görünürlüklü neon odak halkası, masaüstü sesli geri bildirim ipuçları ve `corten-settings` üzerinde özel erişilebilirlik paneli.
- [x] **Pencere Bazlı Arka Plan Bulanıklığı (Per-Window Dual-Kawase Blur - FAZ 25):** Yalnızca panellerde değil, açık şeffaf terminaller (Alacritty, Kitty, Foot vb.), yarı saydam pencereler ve `opacity_percent` kurallı pencerelerin altında GLSL GPU donanım hızlandırmalı Dual-Kawase akrilik bulanıklık, `GlassRegion` SDF yuvarlatılmış köşe kenetlenmesi ve tam ekranda otomatik bypass optimizasyonu.
- [x] **Ağaç Tabanlı Serbest Bölme & Drag-to-Split (Tree-Based BSP Tiling - FAZ 26):** Master-stack döşemeye ek olarak serbest yatay/dikey bölme (BSP), kısayollarla (`Super+V`/`Super+B`) yön seçimi ve pencereleri sürükleyip hedef pencere kenarına bırakarak anında bölme (Drag-to-Split).
- [x] **Derin AccessKit & AT-SPI2 Canlı Nesne Ağacı (Full a11y Object Tree - FAZ 27):** Kompozitör ve kabuk nesnelerinin buton, metin ve girdi ağaçlarını D-Bus AT-SPI2 protokolüne tam metin ağacı olarak bağlama.
- [x] **DRM Renk Yönetimi & HDR/ICC Hazırlığı (`wp_color_management_v1` - FAZ 28):** Monitör ICC profil yükleme ve başlık ayrıştırma, sRGB/DCI-P3/BT.2020 renk uzayı dönüşüm matrisleri, SMPTE ST 2084 (PQ) transfer eğrileri, DRM `HDR_OUTPUT_METADATA` özellik bloğu ve donanımsal `Colorspace` konnektör entegrasyonu; `corten-settings` üzerinden renk uzayı seçimi, HDR toggle anahtarı ve tepe parlaklığı sürgüsü.
- [x] **İnteraktif Screencast & PipeWire Ekran/Pencere Seçim Portalı (`corten-portal` - FAZ 29):** WebRTC/OBS için D-Bus `ScreenCast` portalı, kompozitörden dinamik ekran ve pencere listesi keşfi, interaktif kaynak seçim diyaloğu ve PipeWire video akış düğümü başlatma.
- [x] **İki Yönlü Çoklu-MIME XWayland Pano & Sürükle-Bırak Köprüsü (FAZ 30):** Wayland ve X11 uygulamaları arasında zengin metin (`text/html`), UTF-8 ham metin, resim (`image/png`) ve dosya listesi (`text/uri-list`) için `send_selection` / `new_selection` veri köprüsü ve iki yönlü senkronizasyon.
- [x] **Multi-GPU & PRIME Render Offloading Mimarisi (FAZ 31):** Çift GPU'lu (iGPU + dGPU) sistemlerde `MultiGpuDevice` yapısı, ikincil render düğümleri (`/dev/dri/renderD*`) tespiti ve sıfır kopyalı tampon paylaşımı.
- [x] **Sıfır Kopyalı Donanımsal Video Düzlemleri (Direct Video Overlay Planes - FAZ 32):** Smithay DRM render pipeline'ında `FrameFlags::ALLOW_OVERLAY_PLANE_SCANOUT` ve `ALLOW_PRIMARY_PLANE_SCANOUT_ANY` ile donanım hızlandırmalı video akışlarının doğrudan donanımsal DRM Overlay Plane katmanına aktarımı ve sıfır GL shader ek yükü.
- [x] **Wacom & Grafik Tablet Stylus Desteği (`zwp_tablet_manager_v2` - FAZ 33):** Smithay `delegate_tablet_manager!`, `TabletState`, `TabletSeatTrait` ve Libinput `Event::Tablet` entegrasyonu; basınç (pressure), eğim (tilt), mesafe (distance), silgi ve stylus butonlarının Krita/GIMP/Blender'a sıfır gecikmeyle aktarımı.
- [x] **NVIDIA Explicit Sync Doğrulama & Timeline Semaforları (`linux-drm-syncobj-v1` - FAZ 34):** Modern NVIDIA 555/560+ ve Vulkan oyunları için DRM timeline syncobj ile sıfır flicker senkronizasyonu; hem yerel DRM KMS hem de iç içe (nested) çalışma modunda `supports_syncobj_eventfd` kontrolü ve global tescili.
- [x] **Görsel Çoklu Ekran Topoloji Tuvali (KScreen Paritesi - FAZ 35):** `corten-settings` içinde dinamik görsel KScreen monitör düzenleme tuvali, gerçek zamanlı düşey ofset kaydırıcısı (`offset_y`), üst/orta/alt hizalama butonları ve `corten-compositor` KMS CRTC koordinatlarına piksel-hassas aktarımı.
- [x] **Dinamik Scriptable Eklenti / Widget Motoru (`corten-panel` - FAZ 36):** Waybar ve Polybar standardında JSON (`text`, `tooltip`, `class`, `percentage`) ve düz metin betik applet motoru; dinamik renkli haplar (critical, warning, success, info), zengin üzerine gelme ipuçları (`tooltip`), sol/sağ tıklama eylem tetikleyicileri (`on_click`, `on_right_click`) ile Rust derlemesi gerektirmeyen sonsuz genişletilebilirlik.
- [x] **Derinlemesine Yerel AI Asistanı & Gerçek Zamanlı Akış Motoru (`corten-ai` - FAZ 37):** Evrensel yerel LLM motor keşif adaptörü (Ollama: 11434, llama-server: 8080, AURA-CORE: 8000), SSE / NDJSON protokolleriyle harf harf akışkan token akışı (live token streaming), çok turlu konuşma geçmişi (`Vec<ChatMessage>`), saf Rust RFC tabanlı Base64 kodlayıcı ile ekrandan görsel anlama (Multimodal VLM "Ekranı Analiz Et"), genişletilmiş doğrudan masaüstü eylemleri (disk, terminal, dosyalar, ayarlar, ses, parlaklık, gece ışığı, çalışma alanları) ve akrilik HUD arayüzü.
- [x] **Saf Rust Gömülü Vektör (`corten-vector`), GGUF LLM Çıkarım (`corten-llm`) ve Tam AURA-CORE Orkestrasyonu (`corten-ai/src/aura.rs` - FAZ 38):** Python/PyO3/FastAPI bağımlılığını sıfırlayarak AURA-CORE mimarisinin tamamının doğrudan Corten çalışma alanına (`crates/vector`, `crates/llm`, `corten-ai/src/aura.rs`) entegrasyonu; HNSW küçük-dünya komşuluk grafiği, metadata filtreleme (`==`, `!=`, `>`, `<`, `>=`, `<=`), WAL kalıcılığı, LSH n-gram/kelime semantik metin gömme (`embed_text`), memmap2 sıfır-kopyalı GGUF okuyucu, Q4_K/Q6_K/Q5_0/Q8_0/F16 niceleme, RoPE/RMSNorm/SwiGLU Transformer blokları, kalıcı kullanıcı profili ve uzun süreli vektör hafızası (`user_profile.json` + `memory_vector.wal`), 20 yerleşik ajan aracı (`AuraToolRegistry`), çok adımlı otonom ReAct (`Thought -> Action -> Observation -> Final Answer`) döngüsü, HITL güvenlik kapısı (`hitl_audit.log`) ve `corten-launcher` semantik dosya arama desteği.

---

## Başarı ölçütleri

- 6+ bağımsız, kullanılabilir, belgelenmiş repo
- Temiz bir VM'de tek komutla çalışan kurulum
- Ana repo için 60 saniyelik demo videosu ve en az bir topluluk paylaşımı
- Kendi günlük sistemin: Arch üzerinde tamamen bu masaüstünü kullanıyorsun
