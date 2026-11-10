# Corten test matrisi

## Her değişiklikte

~~~
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p corten-auth -p corten-config -p corten-launcher -p corten-greeter -p corten-panel -p corten-notify -p corten-settings
~~~

corten-lock ve corten-compositor test binary'lerinin link aşaması PAM,
Libseat, Libinput ve DRM geliştirme paketlerine ihtiyaç duyar. Bu paketler
yoksa cargo check yine çalıştırılmalı, link testi bağımlılık kurulumu sonrasına
bırakılmalıdır.

## DRM/TTY smoke

~~~
cargo build -p corten-compositor
tests/tty-smoke.sh
~~~

Script DRM cihazı yoksa kontrollü olarak SKIP döner. Cihaz varsa compositor’ın
başlangıç logunu, permission-denied/panic durumlarını ve SIGTERM kapanışını
kontrol eder.

SIGTERM/SIGINT doğrudan compositor tarafından atomik bir kapanış bayrağına
çevrilir; DRM output’ları, renderer ve Libseat oturumu ana döngü dışında
temizlenir. Bu nedenle smoke testte süreç sonlandırıldıktan sonra
`Corten Compositor DRM engine exited cleanly` satırı aranmalıdır.

## Manuel Wayland matrisi

| Senaryo | Beklenen doğrulama |
| --- | --- |
| Nested compositor | XDG toplevel açılır, tiling ve layer-shell çalışır |
| ext-session-lock | Tüm registered output lock surface’leri gelmeden locked onayı gönderilmez; her lock yüzeyi yalnızca ait olduğu output’ta, normal layer’ların üstünde render edilir |
| PAM başarısız parola | UI hata verir, parola alanı temizlenir |
| Settings save | TOML yazılır, D-Bus reload hatası UI’da görünür |
| Küçük çözünürlük | Settings ve launcher taşmadan açılır |
| Fractional scale | Output scale Wayland output state’inde değişir |
| İkinci monitor | Her bağlı connector için ayrı `wl_output`/CRTC ve frame queue doğrulanır; layout konumu loglanır |
| Input reload | Keyboard layout, Caps remap, pointer speed, natural scroll ve tap-to-click yeniden uygulanır |
| Shared auth lifecycle | Greeter ve lock aynı credential state controller'ını kullanır; başarısız/başarılı denemede secret temizlenir |
| Settings keyboard navigation | Settings sidebar Alt+↑/↓ ile pointer olmadan kategori değiştirir; panel popover Escape ile kapanır |
| Notification lifecycle | Normal bildirimler timeout ile kapanır; critical bildirimler timeout’tan etkilenmez |
| Display reload | Primary/secondary fractional scale ve extend-left/extend-right/mirror arrangement reload sonrası output state’ine uygulanır |
| Corten idle blanking | `power.toml` içindeki ekran uyku süresi compositor reload sonrası uygulanır; DRM’de DPMS kapanır, nested backend’inde output karartılır ve input ile uyanır |
| Corten suspend | `suspend_mins` dolduğunda compositor, systemd-logind D-Bus `Suspend(true)` çağrısı yapar; çağrı başarısızsa journal’a hata yazılır |
| Multi-monitor window ownership | Yeni pencereler tek output’a atanır; Super+Shift+←/→ ile bağlı outputlar arasında taşınır ve duplicate render edilmez |
| Keybinding reload | `compositor.toml` içindeki keybinding chord’ları D-Bus reload sonrası nested ve DRM input path’lerinde uygulanır |

Donanımlı CI runner bulunmadığı için DRM, PAM ve multi-monitor senaryoları
yerel donanım smoke testi olarak tutulur; test sonucu ve GPU/output bilgisi
release notlarına eklenmelidir.

Multi-monitor backend’i connector/CRTC ve output ölçeğini gerçek donanıma
uygular. Her registered output ayrı layout geometry hesaplar; aynı global
workspace pencereleri output başına farklı usable area’ya yerleştirilir.
Gerçek donanımda connector/CRTC ve frame pacing ayrıca doğrulanmalıdır.
