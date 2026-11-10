# Corten Geliştirme Günlüğü (DEVLOG)

Bu günlük, Corten Wayland Masaüstü Ortamı'nın ilk günden son aşamasına kadar alınan kararları, teknik zorlukları ve kat edilen aşamaları belgeler.

---

## [2026-10-29] - Sektör Standartları & Olgun Masaüstü Paketi (Kategori 1 & Kategori 2 Tamamlandı)

Kullanıcı gereksinimleri doğrultusunda sektör standardı masaüstü ortamlarının (GNOME/KDE/COSMIC) tüm "Olmak Zorunda" (Kategori 1) ve "Olmalı" (Kategori 2) özellikleri eksiksiz olarak tamamlandı; Kategori 3 maddeleri ise Backlog'a not edildi:

### 1. Kategori 1 (Olmak Zorunda / Must-Have) Tamamlananlar:
- **Floating vs Tiling Pencere Modu (`corten-compositor`):**
  - `LayoutManager` içine serbest yüzen pencere (`floating_windows`) listesi ve geometrileri entegre edildi.
  - `Super + Shift + Space` global kısayolu ile odaklanılan pencere anında döşeme (tiling) ve serbest yüzme (floating) modları arasında geçiş yapar.
- **İnteraktif Wi-Fi Tarama & Şifre Giriş Modalı (`corten-panel`):**
  - Ağ tarama (`nmcli dev wifi`), sinyal seviyeleri ve kilit durumları Control Center paneline entegre edildi.
  - Tıklanan kablosuz ağ için güvenli şifre giriş popup modalı ve anında bağlantı yönetimi eklendi.
- **Bluetooth Cihaz Yönetimi (`corten-panel`):**
  - Panel Kontrol Merkezi'nden Bluetooth radyo açma/kapama ve bağlı cihazları görüntüleme.
- **XDG Desktop Portal Entegrasyonu (`corten-session`):**
  - Oturum açılışında `dbus-update-activation-environment --systemd WAYLAND_DISPLAY XDG_CURRENT_DESKTOP XDG_SESSION_TYPE` ve portal köprüsü otomatik başlatılır.
- **Sistem Yükleyici & Paketleme Betiği (`install.sh`):**
  - Tüm ikili dosyaları derleyen, `/usr/local/bin` altına kuran ve `/usr/share/wayland-sessions/corten.desktop` Wayland oturum kaydını oluşturan betik eklendi.

### 2. Kategori 2 (Olmalı / Should-Have) Tamamlananlar:
- **PipeWire Ses Çıkışı Seçici (`corten-panel` / Control Center):**
  - `wpctl status` ile mevcut ses çıkış aygıtları (kulaklık, hoparlör, HDMI) listelenir; kullanıcı tek tıkla varsayılan çıkış cihazını (`wpctl set-default`) değiştirebilir.
- **Bildirim Geçmişi Çekmecesi (Notification History Drawer - `corten-notify`):**
  - D-Bus üzerinden `toggle_history` veya panelden tetiklenen, geçmiş bildirimleri listeleyen, tek tek veya topluca temizleme imkanı sunan yan çekmece arayüzü eklendi.
- **Donanım / DDC Monitör Parlaklık Entegrasyonu (`corten-settings`):**
  - Ekran parlaklığı ayarlandığında hem dahili paneller (`brightnessctl`) hem de harici DDC/CI monitörler (`ddcutil`) canlı olarak kontrol edilir.
- **Alt + Tab Pencere Değiştirici (`corten-compositor`):**
  - `Alt + Tab` ve `Alt + Shift + Tab` ile açık pencereler arasında hızlı odak döngüsü sağlandı.

### 3. Kategori 3 (Olsa da Olur / Backlog Not Edilenler):
- `corten-files` dosya yöneticisi, pencere bazlı GPU akrilik bulanıklık, 3 parmak dokunmatik yüzey jestleri ve hava durumu/takvim senkronizasyonu [ROADMAP.md](file:///home/shura/Projects/WaylandDesktopProject/ROADMAP.md) Backlog'una kaydedildi.

### Doğrulama:
- `cargo check --workspace`
- `cargo clippy --workspace -- -D warnings` (Sıfır hata, sıfır uyarı)
- `cargo test --workspace` (Tüm testler başarılı)

---

## [2026-10-29] - Çift Başlatıcı Mimarisi: KDE Application Dashboard (Grid) & Spotlight Modu

Kullanıcılara iki farklı kullanım deneyimini (Spotlight ve KDE Plasma Application Dashboard) aynı anda sunan esnek çift başlatıcı mimarisi geliştirildi:

### Gerçekleştirilen İyileştirmeler
1. **KDE Plasma Tarzı Dev Uygulama Izgarası (Application Dashboard Grid):**
   - 980x640px boyutlarında ferah, 4 sütunlu büyük uygulama kartları (`▶ / ◆` büyük ikonlar, başlık ve açıklama metinleri).
   - Akrilik koyu antrasit buzlu cam kartları, üzerine gelindiğinde Corten pas parıltısı ve klavyeyle anında filtreleme.
2. **Başlatıcı İçinden Anında Tarz Değiştirme:**
   - Üst sekme barının sağına eklenen `⊞ Dashboard View / ☰ Spotlight View` butonu ile kullanıcı tek tıkla arayüzü kompakt Spotlight ile dev ızgara arasında dönüştürebilir.
3. **Panel ve Global Kısayol Entegrasyonu:**
   - Alt paneldeki Başlatıcı bölümüne özel `⊞` (Dashboard) butonu eklendi; doğrudan KDE tarzı ızgarayı açar.
   - `corten-compositor` global kısayolları:
     - `Super + Space / Super + D`: Hızlı kompakt Spotlight arama
     - `Super + A`: KDE Application Dashboard Grid doğrudan tam ekran açılışı
4. **Ayarlar Merkezi (`corten-settings` Launcher Sayfası):**
   - `Default Launcher Style` ayarı eklendi: Kullanıcı kalıcı olarak Spotlight veya Dashboard'u varsayılan seçebilir.
5. **Kalite Doğrulaması:**
   - `cargo check`, `cargo test` ve `cargo clippy --workspace -- -D warnings` sıfır hata/uyarı ile doğrulandı.

---

## [2026-10-29] - Üst Düzey Masaüstü Bütünlüğü: Sistem Tepsisi (SNI Tray), Yaylı Pencere Animasyonları (Spring Physics) & Çoklu Monitör

Olgun masaüstü ortamlarının (GNOME/KDE/COSMIC) seviyesine ulaşmak için hedeflenen 3 kritik eksik hayata geçirildi:

### Gerçekleştirilen İyileştirmeler
1. **Sistem Tepsisi (StatusNotifierItem - SNI Tray - `corten-panel`):**
   * Panelin sağ kısmına, saat modülünün hemen soluna dinamik tepsi alanı eklendi.
   * Steam (`🎮`), Discord/Vesktop (`💬`), Telegram (`✈`) ve OBS Studio (`🔴`) gibi arka planda çalışan popüler masaüstü uygulamalarını otomatik olarak algılar ve tepsiden anında etkileşim sağlar.
2. **Kusursuz Yaylı Pencere Animasyonları (Spring Physics Animations - `corten-compositor`):**
   * Pencerelerin aniden yer değiştirmesi veya kaba kesintiler yerine, 60-120 FPS akıcılığında çalışan dinamik sönümlemeli yay fiziği (`arrange_animated` / Spring stiffness 0.32) geliştirildi.
   * Yeni pencere açıldığında merkezden %92 ölçekten büyüyerek sahneye girer; pencere kapandığında veya oranlar (`Super+H/L`) değiştiğinde pencereler yumuşakça hedeflenen yeni koordinatlarına süzülür.
3. **Çoklu Monitör Derinliği & Bağımsız Ölçekleme (`corten-settings` Displays):**
   * `Displays & Screen` ayar paneline çoklu ekran yönetim kontrolleri eklendi:
     - **Monitör Düzeni:** `Extend Right →`, `← Extend Left` ve `Mirror Display` (Yansıtma) seçenekleri.
     - **Bağımsız Kesirli Ölçekleme:** Birincil (laptop/dahili) ve ikincil (harici HDMI/DisplayPort) monitörler için ayrı ayrı %100, %125, %150 ölçek faktörleri.
4. **Workspace Kalite Doğrulaması:**
   * Tüm workspace `cargo check`, `cargo test` ve `cargo clippy --workspace -- -D warnings` sıfır hata/uyarı ile doğrulandı.

---

## [2026-10-29] - Hızlı Araçlar Paketi: Bluetooth Yönetimi, Ekran Alıntısı (Screenshot) & Rahatsız Etme (DND)

Kullanıcıların en çok talep ettiği temel masaüstü araçları doğrudan sisteme ve Control Center'a entegre edildi:

### Gerçekleştirilen İyileştirmeler
1. **Bluetooth Denetimi (`corten-panel` / Control Center):**
   * Paneldeki Kontrol Merkezi'ne entegre Bluetooth butonu (`ᛒ`) ile tek tıkla Bluetooth radyosunu açma/kapama (`bluetoothctl power on/off`).
   * Bağlı kulaklık veya cihaz adı doğrudan buton üzerinde gerçek zamanlı görüntülenir.
2. **Hızlı Ekran Alıntısı & Kırpma (Screenshot / Snip):**
   * Kontrol Merkezi'ne `📸 Snip` butonu eklendi.
   * `corten-compositor` seviyesinde `PrintScreen` (Tam Ekran) ve `Super + Shift + S` (Bölge Kırpma) global kısayolları tanımlandı.
   * Çekilen ekran görüntüleri doğrudan `~/Pictures/Screenshot_*.png` dosyasına kaydedilirken aynı anda panoya (`wl-copy`) kopyalanır ve masaüstü bildirimi fırlatır.
3. **Rahatsız Etme Modu (Do Not Disturb - DND):**
   * Bildirimleri geçici olarak bastıran, kritik uyarılar hariç ekranı meşgul etmeyen tek tık DND butonu (`🔔` / `🔕`) eklendi.
   * D-Bus bildirim sunucusu (`corten-notify`) DND aktifken bildirimleri arka plan geçmişine alarak sessiz modda çalışır.
4. **Kod Kalitesi & Doğrulama:**
   * Tüm workspace `cargo check`, `cargo test` ve `cargo clippy --workspace -- -D warnings` sıfır uyarı ile doğrulandı.

---

## [2026-10-29] - Modern Başlat Menüsü (`corten-launcher`): Kategori Filtreleme, Profil & Hızlı Sistem Yönetimi

Genel masaüstü kullanıcılarına ve profesyonellere hitap eden, Spotlight/Raycast hızında modern başlat menüsü (`corten-launcher`) geliştirildi:

### Gerçekleştirilen İyileştirmeler
1. **Kategoriye Göre Anında Filtreleme (App Category Pills):**
   * Başlat menüsü üst kısmına arama kutusunun hemen altına yerleştirilen hap (pill) butonlar: `All`, `Internet`, `Office`, `Media`, `Development`, `System`.
   * XDG `.desktop` standart kategorilerine göre uygulamaları otomatik sınıflandırır; klavye odağını bozmadan fareyle veya kısayollarla hızlı geçiş sağlar.
2. **Kullanıcı Profili & Entegre Hızlı Güç/Sistem Çubuğu (Footer User & Quick Actions):**
   * Başlat menüsünün alt barına aktif oturum kullanıcısını (`👤 <username>`) gösteren minimalist profil hapı entegre edildi.
   * Sağ tarafa Ayarlar (`⚙ Settings`), Ekran Kilitleme (`🔒 Lock`) ve Güç (`⏻ Power`) hızlı eylem butonları eklendi.
3. **Kusursuz Klavye & Fare Çift Uyumu:**
   * Spotlight tarzı `Tab`, `↑ / ↓`, `Enter`, `Esc` tuşlarıyla ultra hızlı gezinme korunurken, fareyle kategori filtreleme ve hızlı sistem komutlarına tek tıkla ulaşım bir araya getirildi.
4. **Sıfır Uyarı & Saf Rust Güvenilirliği:**
   * `-D warnings` ile tam uyumlu, runtime panic riski taşımayan, saf Rust `iced` + `tiny-skia` mimarisi.

---

## [2026-10-29] - Gelişmiş Alt Panel & Dock Mimarisi: Hızlı Güç Menüsü & Masaüstünü Göster (Peek Desktop)

Kullanıcıların alt panel (dock) tarafındaki en kritik talepleri hayata geçirildi:

### Gerçekleştirilen İyileştirmeler
1. **Başlat Butonu Hızlı Güç Menüsü (Quick Power Menu):**
   * Başlat butonunun hemen yanına yerleştirilen özel güç butonu ile tek tıkla açılan kompakt açılır kart (popup card).
   * Kullanıcının doğrudan oturumu kilitleme, oturumdan çıkış, terminal açma, yeniden başlatma ve sistemi kapatma eylemlerine anında erişmesini sağlar.
2. **Hızlı Masaüstünü Göster (Show / Peek Desktop):**
   * Panelin en sağ ucuna yerleştirilen minimalist Windows/macOS tarzı şerit buton ile tek tıkla pencereleri arka plana atıp masaüstünü, duvar kağıdını ve widget'ları öne getirme imkanı sağlandı.
3. **Akıllı Boyutlandırma & Görsel Denge:**
   * Dock kapsülü 780px genişliğe ve 68px yüksekliğe optimize edildi; açık olan popup kartlarına (Control Center, Power Menu) göre layer yüksekliği dinamik olarak `380px`'e genişler.
   * Açık ve odaklanmış uygulamalar altlarındaki parlak Corten turuncusu çizgiler (pills) ve beyaz noktalar (dots) ile gerçek zamanlı gösterilmeye devam eder.

---

## [2026-10-29] - Kapsamlı Ayarlar Merkezi (`corten-settings` v0.2.0): Canlı Filtreleme & Giriş, Ekran, Ses ve Güç Yönetimi

Tüm masaüstü ekosistemini tek bir noktadan yöneten **`corten-settings`** modern işletim sistemi standartlarına ulaştırıldı ve 12 kategorilik eksiksiz bir mimariye kavuşturuldu.

### Gerçekleştirilen İyileştirmeler
1. **Canlı Tercih Arama (Global Settings Filter):**
   * Sol gezinme çubuğunun tepesine entegre edilen anlık arama motoru ile ("ses", "klavye", "pil", "widget", "gap" vb.) kategoriler gerçek zamanlı filtrelenir.
2. **Klavye & Giriş Aygıtları (Input / Keyboard & Mouse):**
   * XKB klavye düzeni seçimi (TR-Q / US), Dokunmatik yüzey doğal kaydırma (Natural scrolling), Dokunarak tıklama (Tap-to-click) ve yazılımcılar için vazgeçilmez olan ergonomik `CapsLock -> Ctrl/Esc` eşleme ayarları eklendi.
3. **Ekran & Çözünürlük (Displays & HiDPI):**
   * Wayland kesirli ölçekleme (Fractional Scaling: %100, %125, %150), Donanım parlaklık sürgüsü ve göz yorgunluğunu önleyen Gece Işığı (Night Light / Mavi Işık Filtresi) entegre edildi.
4. **Masaüstü Araçları Yönetimi (Desktop Widgets Desklets):**
   * `corten-widgets` için Canlı Ses Dalga Görselleştiricisi, Sistem Donanım Monitörü ve Masaüstü Karalama Not Defteri kartlarının açılıp kapanması ve konumlandırılması sağlandı.
5. **Ses & Güç Yönetimi (Sound & Power Profiles):**
   * Master çıkış ve mikrofon giriş seviyeleri, anında sessize alma ve sistem enerji profilleri (Power Saver, Balanced, Performance) ile ekran boşta uyku süreleri kontrol edilebilir hale getirildi.
6. **D-Bus IPC Hot-Reload Entegrasyonu:**
   * Yapılan tüm değişiklikler disk üzerindeki TOML dosyalarına atomik yazılırken çalışan tüm masaüstü süreçlerine D-Bus üzerinden sinyal gönderilerek oturum kapatmaya gerek kalmadan canlı senkronize edilir.

---

## [2026-10-29] - Masaüstü Araçları (`corten-widgets`): Canlı Ses Spektrumu / Dalga Efekti & Sistem & Karalama Notları

Masaüstü arka plan katmanında (`Layer::Bottom`) çalışan, pencere kullanımını asla engellemeyen ve sisteme sıfır yük getiren yeni **`corten-widgets`** bileşeni geliştirildi.

### Gerçekleştirilen İyileştirmeler
1. **Canlı Müzik & Dalga Efekti (Now Playing & Waveform Visualizer):**
   * Linux standart `org.mpris.MediaPlayer2` (Spotify, Chromium, YouTube, Firefox vb.) üzerinden çalan şarkı adını, sanatçıyı ve oynat/duraklat/geç kontrol düğmelerini barındırır.
   * `iced::widget::canvas` ile 30 FPS yumuşak geçişli, müziğin ritmine göre dalgalanan dinamik ses frekans/ekolayzır çubukları (`WaveformVisualizer`) geliştirildi. Müzik durduğunda minimal bekleme moduna geçer.
2. **Hafif Sistem & Donanım Gözlemcisi (SysInfo Desklet):**
   * Linux `/proc/stat` ve `/proc/meminfo` çekirdek dosyalarını doğrudan ayrıştırarak harici komut forklamadan anlık CPU kullanım yüzdesini ve RAM (MB / %) tüketimini gösterir.
3. **Masaüstü Karalama / Görev Notları (Desktop Scratchpad):**
   * Masaüstü üzerinden tek tıkla düzenlenebilen, `XDG_DATA_HOME` altında kalıcı saklanan hafif not defteri kartı.
4. **Oturum Orkestrasyonu (`corten-session`):**
   * `corten-widgets` bileşeni `corten-session` başlangıç süreçlerine eklendi; masaüstü oturumu açıldığında otomatik olarak arka planda çalışır.

---

## [2026-10-29] - Geliştirici Odaklı Masaüstü Deneyimi: Hızlı Proje Arama & Caffeine & Port Monitörü

Yazılımcılar ve sistem geliştiricileri için masaüstü verimliliğini üst seviyeye taşıyan iki kritik özellik hayata geçirildi:

### Gerçekleştirilen İyileştirmeler
1. **`corten-launcher` Geliştirici Proje / Repo Arama (`LauncherMode::Projects`):**
   - Kullanıcının `~/Projects`, `~/code`, `~/src`, `~/Workspace`, `~/ghq` gibi yaygın geliştirme dizinlerini otomatik tarayan hafif ve optimize `scan_dev_projects()` motoru eklendi.
   - Proje tipini (`Cargo.toml` -> Rust, `package.json` -> Node.js, `go.mod` -> Go, `requirements.txt`/`pyproject.toml` -> Python, `.git` -> Git Repo) otomatik algılayıp rozetlerle gösterir.
   - Başlatıcıda `:p` veya `Tab` tuşu ile doğrudan Projeler sekmesine geçiş yapılır; fuzzy search (`nucleo`) ile anında filtrelenir.
   - Seçilen projede `Enter` tuşuna basıldığında tercih edilen IDE (`code`, `cursor`, `zed`, `subl` veya terminal `nvim`) otomatik açılır.
   - Sağ taraftaki dinamik önizleme panelinde projenin tam yolu, projenin dili ve Git repo durumu görüntülenir.
2. **`corten-panel` Control Center Caffeine (☕ No-Sleep) & Dinamik Port İzleyici:**
   - Control Center penceresine geliştiriciler için **Caffeine (Keep-Awake)** modu eklendi. Uzun derleme, test veya model eğitimi esnasında ekranın kapanmasını ve sistemin uykuya geçmesini `systemd-inhibit` ile tek tıkla engeller/açar.
   - `/proc/net/tcp` ve `/proc/net/tcp6` doğrudan çekirdek seviyesinde okunarak (harici komut çalıştırmadan) makinede o an açık olan yerel geliştirme portları (`:3000`, `:8080`, `:5173` vb.) Control Center'da canlı listelenir.

---

## [2026-10-29] - CPU & Bellek Optimizasyonları ve Arayüz Görsel Cilalama (Polish)

Masaüstü bileşenlerinin sıfır busy-spin CPU kullanımı, Unix felsefesine uygunluk ve modern masaüstü standartlarında üst düzey görsel cilalama çalışmaları tamamlandı.

### Gerçekleştirilen İyileştirmeler
1. **CPU & Performans Optimizasyonları:**
   - `corten-compositor`: Event loop içindeki busy-spin ve yüksek CPU kullanımını önlemek amacıyla 16ms'lik akıllı frame pacing / sleep mantığı eklendi.
   - `corten-panel`: Ağ bağlantı durumu için harici `nmcli` fork/exec çağrısı kaldırıldı; Linux `/sys/class/net/*/operstate` sysfs düğümleri üzerinden sıfır fork, doğrudan çekirdek seviyesi okuma yapıldı.
2. **Layer-Shell & tiny-skia Güvenilirliği:**
   - `layershellev`'in configure öncesi 0x0 boyut atamasından kaynaklanan `tiny_skia::Mask::new(0, 0)` paniği `patches/iced_layershell` üzerinde min(1) clamp ile çözüldü. Panel ve pencereler hem KWin hem nested ortamda sıfır panik ile kararlı hale getirildi.
3. **Panel & Dock Control Center Popup:**
   - Panel saat/tarih modülü interaktif hale getirildi; tıklandığında dock üzerinde ses seviyesi ve oturum kontrol butonları (Kilit, Çıkış, Yeniden Başlat, Kapat) içeren yarı saydam cam Control Center popup kartı açılıyor.
   - Katman yüksekliği dinamik olarak `80px` ile `320px` arasında wlr-layer-shell `SizeChange` ile uyumlu boyutlandırılıyor.
4. **Spotlight Launcher (`corten-launcher`) Split-View Görünümü:**
   - Raycast ve macOS Spotlight tarzı split-view arayüzüne geçildi.
   - Solda sonuç listesi, sağda ise seçili uygulamanın büyük Lucide ikonu, kategorisi, exec komutu, desktop ID'si ve terminal durumu gibi meta verileri gösteren dinamik önizleme paneli eklendi.
   - Pano modunda çok satırlı içerik önizlemesi ve karakter sayısı bilgisi yerleştirildi.
5. **Bildirim Sistemi (`corten-notify`) İlerleme Çubuğu ve Aksiyonlar:**
   - Toast bildirim kartlarının altına otomatik kapanma süresini gösteren dinamik ilerleme çubuğu (timeout progress bar) yerleştirildi.
   - Bildirime bağlı interaktif aksiyon butonları (`actions`) ve doğrudan kart üzerinden aksiyon tetikleme desteği sağlandı.
6. **Ayarlar (`corten-settings`) Canlı Minyatür Masaüstü Önizlemesi:**
   - Görünüm sekmesine aktif renk paleti, pencere dekorasyonu ve dock stilini anlık simüle eden canlı minyatür bir masaüstü tuvali (miniature desktop canvas preview) eklendi.

---

## [2026-10-28] - D-Bus IPC Canlı Ayar Yenileme (Hot-Reload) Mimarisi

Ayarlar uygulamasından (`corten-settings`) yapılan değişikliklerin diske yazılmasının yanı sıra, çalışan tüm masaüstü bileşenlerine anında D-Bus üzerinden sinyal gönderilerek gecikmesiz canlı yenilenmesi (hot-reload) sağlandı.

### Gerçekleştirilen İyileştirmeler
1. **`corten-ipc` Arayüz Genişletmesi:**
   - `LauncherControl`, `NotificationControl` ve `PanelControl` arayüzlerine `reload_config()` metodu eklendi.
   - Compositor için `org.corten.Compositor` D-Bus proxy arayüzü (`CompositorControl`) tanımlandı.
2. **`corten-panel`:**
   - `org.corten.Panel` servisi `PanelIpcCommand::ReloadConfig` komutunu `CortenPanelApp` olay döngüsüne aktararak `panel.toml` dosyasını anında yeniden yükler.
   - Başlatıcı butonu, ses ve ağ modülleri konfigürasyon bayraklarına (`show_launcher_button`, `show_audio`, `show_network`) dinamik bağlandı.
3. **`corten-launcher` & `corten-notify`:**
   - D-Bus üzerinden gelen `ReloadConfig` çağrısı ile diskteki güncel TOML dosyalarını anında belleğe alır.
4. **`corten-compositor`:**
   - `org.corten.Compositor` D-Bus servisi ile `compositor.toml` içindeki `gap` (tiling aralığı) değerini anında okuyarak çalışma alanını anlık olarak yeniden boyutlandırır.
5. **`corten-settings`:**
   - "Save Settings" tıklandığında TOML dosyalarını günceller ve eş zamanlı olarak arka planda tüm aktif servislere D-Bus `reload_config()` çağrısı yaparak masaüstünü anında senkronize eder.

---

## [2026-10-28] - Faz 9: Birleştirilmiş Oturum Orkestratörü — `corten-session` (v0.1.0)

Corten Desktop projesinin tüm modüllerini tek bir oturum altında toplayan ve yöneten **`corten-session`** orkestratörü tamamlandı.

### Önemli Başarılar ve Mimari Detaylar
1. **Süreç Denetimi ve Yaşam Döngüsü (Process Supervision):**
   - Tek bir `corten-session` komutu ile `corten-compositor` başlatılır.
   - Wayland soketinin (`WAYLAND_DISPLAY=corten-0`) soket dosyasının açıldığını bekler ve tespit eder.
   - Soket hazır olduğunda `corten-panel`, `corten-notify` ve pano dinleyicisini (`wl-paste --watch cliphist store`) otomatik olarak arka planda ayağa kaldırır.
   - Oturum kapandığında veya `SIGINT`/`SIGTERM` sinyali geldiğinde tüm alt süreçleri zarifçe (graceful shutdown) sonlandırır.
2. **Klavye Kısayolları Entegrasyonu:**
   - `Super + Return`: Terminal (`alacritty` / `foot` / `kitty` / `konsole`).
   - `Super + Space` veya `Super + D`: `corten-launcher` Spotlight arama ve pano modalı.
   - `Super + I` veya `Super + ,`: `corten-settings` modern ayarlar merkezi.
   - `Super + Q`: Aktif pencereyi kapatma.
   - `Super + J` / `Super + K`: Pencereler arası odak değişimi.
   - `Super + H` / `Super + L`: Master pencere oranı genişletme/daraltma.
3. **Display Manager & Wayland Oturum Dosyası:**
   - `assets/corten.desktop` Wayland oturum dosyası hazırlandı (`/usr/share/wayland-sessions/corten.desktop` için).

---

## [2026-10-28] - Faz 8: Ayarlar Uygulaması — `corten-settings` (v0.1.0)

Corten Desktop ekosisteminin tüm modüllerini tek bir grafik arayüzden yöneten, anında yapılandırma senkronizasyonu sağlayan **`corten-settings`** uygulaması hayata geçirildi.

### Önemli Başarılar ve Mimari Detaylar
1. **Merkezi XDG TOML Entegrasyonu:**
   - `~/.config/corten/` altındaki `appearance.toml`, `panel.toml`, `launcher.toml`, `notify.toml` ve `compositor.toml` dosyaları `ConfigManager<T>` ile otomatik olarak okundu ve tek tıkla kaydedildi.
   - Faz 1'deki canlı `notify` izleyicisi sayesinde ayarlar arayüzünden yapılan bir değişiklik anında panel, başlatıcı ve bildirim sunucusuna yansıyor.
2. **Kategorik Sidebar Gezinimi:**
   - **Görünüm (Appearance):** Koyu Antrasit / Krem Açık mod seçimi ve duvar kağıdı yolu girişi.
   - **Panel & Dock:** Üst panel / Alt dock konumu, bar görsel stili (Floating, Islands, Flush) ve modül görünürlük kutuları (Launcher, Audio, Network, Battery, Autohide).
   - **Başlatıcı (Launcher):** Spotlight modal genişlikleri (600px / 750px) ve pano geçmişi entegrasyon ayarı.
   - **Bildirimler (Notifications):** Toast süreleri (3s, 5s, 8s) ve kart genişlikleri.
   - **Pencere Yöneticisi (Compositor):** Tiling boşluk payı (`gap`) ayarları (0px, 12px, 20px).
   - **Hakkında (About):** Corten sürüm ve Unix felsefesi bilgileri.
3. **Tutarlı Corten Tasarım Dili:**
   - Lucide ikonları, Corten Rust aksan renkleri ve WCAG AA kontrast kurallarına tam uyumlu Iced widget stilleri kullanıldı.

---

## [2026-10-28] - Faz 7: Dinamik Tiling Wayland Compositor — `corten-compositor` (v0.1.0)

Corten Desktop ekosisteminin bağımsız pencereleri yöneten ve masaüstünü bir araya getiren merkezi compositor'ı olan **`corten-compositor`** (Smithay tabanlı) hayata geçirildi.

### Önemli Başarılar ve Mimari Kararlar
1. **Dinamik Master-Stack Tiling Düzeni:**
   - XDG toplevel pencerelerini otomatik olarak 55/45 oranında sol ana pencere (Master) ve sağ dikey yığın (Stack) olarak dizen dinamik düzenleme algoritması (`LayoutManager`) geliştirildi.
   - Pencereler arası 12px Corten tasarım boşluğu (`gap`) uygulandı ve pencereler kapandığında/açıldığında geometri anında yeniden hesaplandı.
2. **`wlr-layer-shell` Tam Entegrasyonu:**
   - `corten-panel`, `corten-launcher`, `corten-lock` ve `corten-notify` bileşenlerinin compositor üzerinde doğru Z-order ile (Background, Bottom, Windows, Top, Overlay) çizilmesi sağlandı.
   - `LayerMap::non_exclusive_zone()` ile panelin ayırdığı üst/alt alanlar hesaplanıp tiling pencerelerinin çalışma alanı dinamik olarak korundu (pencereler panelin altına kaçmıyor).
3. **Klavye Kısayolları & Fare Etkileşimi:**
   - Pencere ve katman koordinat dönüşümlerinde global origin (`surface_origin`) hesaplamasıyla alt yüzeyler (menüler, düğmeler, popuplar) üzerinde tam isabetli tıklama ve gezinme sağlandı.
4. **Donanımsal GLSL Fragment Shader ile Neon Glow (Işık Yayılımı):**
   - Smithay `PixelShaderElement` ve özel GLES fragment shader'ı ([glow_shader.rs](file:///home/shura/Projects/WaylandDesktopProject/corten-compositor/src/glow_shader.rs)) entegre edildi.
   - 2D Signed Distance Field (`sd_rounded_box`) ve Gaussian üstel sönümlenme ($e^{-d^2}$) ile aktif pencere arkasında 16px yayılımlı, Corten Rust (`#D96B43`) tonunda ve yumuşak kavisli, donanımsal olarak hızlandırılmış gerçek bir neon ışık halesi hayata geçirildi.
   - `Super + Return`: Başlatıcı / terminal tetikleyici.
   - `Super + Q`: Aktif pencereyi kapatma (`send_close`).
   - `Super + J` / `Super + K`: Pencereler arasında döngüsel odak geçişi.
   - `Super + H` / `Super + L`: Master pencere oranını dinamik daraltma/genişletme (0.2 - 0.8 aralığı).
   - Fare imleci ile pencereye tıklandığında otomatik aktif etme ve odak aktarımı.
4. **Çok Katmanlı Popup ve Menü Yönetimi:**
   - `PopupManager` entegre edilerek pencerelerin açtığı sağ tık menüleri, dropdown'lar ve layer-shell bildirimleri pencerelerin üstünde düzgünce render edildi.

---

## [2026-10-28] - Faz 3: Uygulama Başlatıcı ve Pano Geçmişi — `corten-launcher` (v0.1.0)

Corten Desktop projesinin klavye odaklı, Spotlight/Raycast tarzı uygulama başlatıcısı ve pano yöneticisi olan **`corten-launcher`** tamamlandı.

### Önemli Başarılar ve Karşılaşılan Zorluklar
1. **Raycast / Spotlight Üst Konumlandırma & Sıfır Zıplama:**
   - Katman ayarı `Anchor::Top` ve `margin: (140, 0, 0, 0)` olarak sabitlendi.
   - Farklı içerik boyutuna sahip sekmeler (`Apps` ve `Clipboard`) arasında geçiş yapıldığında pencerenin dikeyde zıplama sorunu ortadan kaldırıldı.
2. **Çift Modlu Çalışma & Pano Geçmişi (`cliphist`):**
   - XDG `.desktop` ayrıştırıcısı ile sistem uygulamaları bulundu ve `nucleo` bulanık arama algoritması ile filtrelendi.
   - `Tab` tuşu ile `cliphist` pano geçmişine anında geçiş sağlandı. Seçilen öğe `cliphist decode | wl-copy` üzerinden Wayland panosuna geri yüklendi.
   - Sistemde `cliphist` veya `wl-clipboard` olmadığında çökme yerine kullanıcı dostu sarı uyarı kartı gösterildi.
3. **Ultra-Minimalist Kaydırma & Senkron İlerleme:**
   - Iced `scrollable` widget'ı 4px ultra-ince bir kaydırma çubuğu ile özelleştirildi.
   - Ok tuşlarıyla (`↑` / `↓`) gezinirken seçilen öğe görünür alanın dışına çıktığında `scrollable::snap_to` ile kaydırma çubuğu anında ve orantılı olarak seçili öğeye odaklandı.
4. **Maksimum Performans & Sıfır Gecikme:**
   - Klavye girdi akışı senkronize edildi ve `--release` derlemesi ile sıfır gecikmeli, akıcı deneyim sağlandı.

---

## [2026-10-29] - Faz 9: Oturum Orkestrasyonu, IPC Hot-Reload & Layer-Shell Çözünürlük Çözümü

Bugün Corten Desktop projesinde oturum orkestrasyonu (`corten-session`), bileşenler arası canlı D-Bus IPC yapılandırma yenilemesi ve Wayland layer-shell protokolünde hayati bir upstream hata düzeltmesi tamamlandı.

### Önemli Başarılar ve Teknik Kararlar
1. **Canlı D-Bus IPC Hot-Reload:**
   - `corten-settings` üzerinden kaydedilen değişikliklerin anında ilgili servislere iletilmesi için D-Bus servisleri kuruldu.
   - `corten-panel`, `corten-compositor`, `corten-launcher` ve `corten-notify` servislerine `reload_config` arayüzü eklendi. Kullanıcı ayarları kaydettiği anda compositor pencere boşluklarını (`gap`), panel ise stillerini anında güncelliyor.
2. **Dinamik Ekran Çözünürlüğü Desteği:**
   - Panel (`corten-panel`), ekranın tam genişliğini dinamik olarak dolduracak şekilde (`Length::Fill` + `Anchor::Bottom | Left | Right`) yapılandırıldı; macOS dock tarzı kapsül `Length::Fixed(760.0)` olarak merkeze hizalandı. Böylece FHD, 2K, 4K ve dinamik pencere boyutlarında optimum hizalama sağlandı.
3. **`iced_layershell` & `tiny-skia` Sıfır Boyut Maske Hatasının Çözümü:**
   - `iced_layershell` istemcisinin sunucudan ilk `configure` olayını beklemeden önce `State::new()` sırasında `(0, 0)` boyut alarak `tiny_skia::Mask::new(0, 0)` üzerinde paniklemesi kökünden incelendi.
   - `patches/iced_layershell` içinde yüzey boyutu `max(1)` ile sınırlandırıldı ve `[patch.crates-io]` ile yamalandı. Panel hem KWin hem nested compositor (`corten-compositor`) altında yazılım renderer'ı (`tiny-skia`) ile çökmeden kararlı çalışır hale getirildi.
4. **Oturum Süreç Denetleyicisi (`corten-session`):**
   - Tek bir komutla (`corten-session`) `corten-compositor`, `corten-panel`, `corten-notify` ve pano yöneticisi (`cliphist`) senkronize olarak başlatıldı.
   - Compositor Wayland soketinin hazır olmasını bekleyen (`wait_for_wayland_socket`) ve kapanışta tüm alt süreçleri (`SIGTERM`) temizleyen denetim döngüsü uçtan uca doğrulandı.

---

## [2026-10-28] - Faz 2: İlk Bağımsız Ürün Doğdu — `corten-notify` (v0.1.0)

Corten Desktop projesinin ilk çalışan, bağımsız masaüstü aracı olan **`corten-notify`** başarıyla tamamlandı ve test edildi.

### Önemli Başarılar ve Karşılaşılan Wayland Zorlukları
1. **`wlr-layer-shell` Entegrasyonu:**
   - Bildirim pencereleri `iced_layershell` ile ekranın sağ alt köşesine yerleştirildi.
   - **Görünmez Blokajın Aşılması:** Başlangıçta sabit 600px katman boyutu verildiğinde, pencere şeffaf olsa bile Wayland compositor'ın (KWin) o alanı girdi yüzeyi sayıp tıklamaları bloke ettiği fark edildi. 
   - `LayershellCustomActions::SizeChange` aksiyonu ile katman boyutu **bildirim yokken 1x1 piksel** (Wayland protokolünün sıfır genişlik kuralına uygun) yapıldı. Bildirim geldiğinde tam kart boyutuna genişleyip bitince tekrar 1x1'e dönerek **sıfır ekran blokajı** sağlandı.
2. **`org.freedesktop.Notifications` Standart D-Bus:**
   - `Notify`, `CloseNotification`, `GetCapabilities`, `GetServerInformation` metotları uygulandı.
   - Sistemde KDE/GNOME gibi başka bir bildirim servisi çalışırken çakışmayı önlemek için fallback desteği ve `--test` bayrağı eklendi.
3. **İkon Entegrasyonu:**
   - `iced::font::load(LUCIDE_FONT_BYTES)` asenkron font yükleyicisi ile Lucide ikonları başarıyla kartlara entegre edildi.
4. **Kapsam Yönetimi:**
   - İlkelerimize sadık kalındı: Fade-in animasyonları ve geçmiş modalı `v0.2.0`'a devredildi; `v0.1.0` çalışan ve güvenilir bir ürün olarak kapatıldı.

---

## [2026-10-28] - Faz 1: Temel Kütüphaneler Tamamlandı & Önizleme Doğrulandı

Faz 1 kapsamında projenin bel kemiğini oluşturacak 3 temel kütüphane ve canlı önizleme uygulaması geliştirildi.

### Yapılanlar
1. **`corten-theme`:**
   - Koyu Antrasit / Corten Pası / Krem odaklı Corten renk paleti (Dark & Light) tanımlandı.
   - 12px köşe yuvarlaklığı (`Radius::MD`), 4px-48px `Spacing` ve tipografi ölçekleri kuruldu.
   - `lucide.ttf` fontu doğrudan kütüphaneye gömüldü.
   - **Erişilebilirlik:** WCAG AA (4.5:1) normal metin kontrast oranını denetleyen matematiksel birim testi eklendi.
2. **`corten-config`:**
   - `~/.config/corten/<bileşen>.toml` standart yol çözücüsü ve `ConfigManager<T>` jenerik yapısıyla `notify` canlı izleme kuruldu.
3. **`corten-ipc`:**
   - `org.corten.*` D-Bus arayüz proxy'leri (`LauncherControl`, `NotificationControl`, `PanelControl`) yazıldı.
4. **`theme-preview`:**
   - Iced 0.13 uygulaması olarak çalıştırıldı ve görsel temizlik doğrulandı.

---

## [2026-10-28] - Faz 0: Temeller Atıldı, Corten Doğdu

Bugün sıfırdan, parça parça inşa edilecek ve Unix felsefesine sadık kalacak Wayland masaüstü ortamımızın temellerini attık.
