# Corten kurulum ve çalışma önkoşulları

## Derleme

Rust stable, Cargo ve sistemde libpam, libinput, libseat, libdrm, libgbm,
libudev, libxkbcommon ve EGL/OpenGL geliştirme dosyaları gerekir.

Ubuntu/Debian tabanlı sistemlerde:

~~~
sudo apt install build-essential pkg-config libpam0g-dev libinput-dev \
  libseat-dev libdrm-dev libgbm-dev libudev-dev libxkbcommon-dev \
  libegl1-mesa-dev libgl1-mesa-dev greetd
~~~

DRM/TTY çalıştırması için runtime tarafında ayrıca `libseat.so.1`,
`libinput.so.10` ve uyumlu Mesa/GLIBC sürümleri bulunmalıdır. Bu kütüphaneler
yoksa `tests/tty-smoke.sh` testi kontrollü olarak `SKIP` döndürür.

Çalışma zamanı için pipewire, wireplumber, wpctl, power-profiles-daemon,
brightnessctl, grim, slurp, wl-clipboard ve isteğe
bağlı cliphist kurulur.

Ekran paylaşımı/capture tüketicisi bağımsız `corten-capture` crate'idir ve
`cargo build -p corten-capture --features pipewire` ile derlenir. Bu backend
için PipeWire geliştirme paketleri, `pkg-config` ve `libclang` gerekir. Temel
Corten build'i bu geliştirme bağımlılıklarına ihtiyaç duymaz.
Portal oturumu `corten-services::portal::start_screencast_session` ile
başlatılır; dönen node ID `corten_capture::start` fonksiyonuna verilir.
Frame'ler bounded kanal üzerinden `FramePump` ve bir `FrameConsumer` ile
tüketilir. Capture durdurulmadan önce pump, ardından portal session kapatılır.
Gerçek video çıktısı için corten-capture crate'inin ffmpeg feature'ı
kullanılabilir: cargo build -p corten-capture --features ffmpeg. Bu backend
FFmpeg'i shell kullanmadan doğrudan başlatır ve BGRx frame'lerini H.264
çıktısına aktarır; sistemde ffmpeg binary'si bulunmalıdır.
Hazır kayıt komutu için:

~~~
cargo run -p corten-capture-session --features ffmpeg -- output.mp4
cargo run -p corten-capture-session --features ffmpeg -- --window --fps 60 window.mp4
cargo run -p corten-capture-session --features ffmpeg -- --stream 1 monitor.mp4
cargo run -p corten-capture-session --features ffmpeg -- --all monitors.mp4
~~~

Komut Ctrl+C ile durdurulduğunda encoder, PipeWire ve portal session sırasıyla
kapatılır.
Panel ve diğer Corten bileşenlerinin kullanacağı kalıcı Capture D-Bus servisi:

~~~
cargo run -p corten-capture-session --bin corten-capture-service --features ffmpeg
~~~

Servis org.corten.Capture üzerinde start/stop/status çağrılarını sunar;
panel bu arayüzü kullanır ve PipeWire/FFmpeg'e doğrudan bağlanmaz.
Ekran ölçeği, arrangement, mode-set ve idle güç durumu Corten compositor
tarafından uygulanır; DRM backend’inde idle durumunda gerçek DPMS kapatma,
nested backend’inde ise güvenli karartma kullanılır. Başka bir masaüstü
ortamının ayar daemon’u gerekmez.
Suspend zaman aşımı, compositor idle durumu üzerinden systemd-logind’in
`org.freedesktop.login1` D-Bus arayüzüyle uygulanır.

## Kurulum

~~~
./install.sh
~~~

Script corten-greeter dahil tüm binary'leri /usr/local/bin altına ve
corten.desktop dosyasını Wayland session dizinine kurar.

Kurulum ayrıca `corten-session.service` adlı isteğe bağlı systemd user unit’i
kurar. Display manager/greetd session zaten `corten-session` başlatıyorsa unit’i
etkinleştirmeyin; bağımsız user-session kullanımında:

~~~
systemctl --user daemon-reload
systemctl --user enable --now corten-session.service
systemctl --user status corten-session.service
~~~

## greetd

Greeter yalnızca greetd tarafından başlatıldığında gerçek oturum açar.
/etc/greetd/config.toml için örnek:

~~~
[terminal]
vt = 1

[default_session]
command = "corten-greeter"
user = "greeter"
~~~

Preview modu yalnızca geliştirme amacıyla CORTEN_GREETER_PREVIEW=1 ile
açılabilir; production ortamında GREETD_SOCK yoksa login reddedilir.

PAM stack'i dağıtıma göre değişir. corten-lock, login PAM service'ini kullanır;
sistemde /etc/pam.d/login bulunmalı ve policy yerel güvenlik politikasına göre
doğrulanmalıdır.

## TTY/DRM

corten-session bir greetd session veya doğrudan TTY içinden çalıştırılabilir.
Kullanıcının seat üzerinde Libseat erişimi olmalı; DRM cihazı başka bir
compositor tarafından tutulmamalıdır. İlk smoke test:

~~~
RUST_LOG=info corten-compositor --drm
~~~

Çıkış için Ctrl+Alt+Backspace veya Super+Shift+Escape kullanılır.

## Merkezi kısayollar

Kısayollar `~/.config/corten/compositor.toml` içindeki `keybindings`
alanlarından okunur ve compositor D-Bus reload sonrasında nested/DRM input
akışlarına uygulanır. Chord biçimi `Super+Shift+S` şeklindedir. Settings
uygulamasındaki Kısayollar sayfası çalışan sözleşmeyi gösterir.

## Sağlık ve loglar

Session supervisor desktop child'larını izler ve her bileşeni en fazla üç kez
yeniden başlatır. Compositor kapanırsa tüm child'lar SIGTERM ile kapatılır.

~~~
journalctl --user -f
RUST_LOG=corten_compositor=debug,corten_session=debug corten-session
~~~

