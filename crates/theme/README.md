# Corten Theme (`corten-theme`)

Corten Masaüstü Ortamı için ortak tasarım jetonları, renk paletleri ve `iced` stil yardımcıları kütüphanesi.

## Özellikler
- **Corten Renk Paleti:** Koyu (Antrasit / Corten Pası / Sıcak Krem) ve Açık modlar.
- **Tasarım Jetonları:**
  - `Spacing`: 4px - 48px aralığında tutarlı boşluk ölçeği.
  - `Radius`: Modern 12px varsayılan köşe yuvarlaklığı (`Radius::MD`).
  - `Typography`: Inter ve JetBrains Mono boyutlandırma ölçekleri.
- **Iced Entegrasyonu:** Kartlar, birincil (primary) ve ikincil butonlar için hazır stil fonksiyonları.
- **Hafif Bağımlılık:** Ağır D-Bus veya sistem kütüphanelerine bağımlı değildir; saf ve hızlıdır.

## Kullanım

```rust
use corten_theme::{Palette, ThemeMode, Spacing, Radius};

let palette = Palette::for_mode(ThemeMode::Dark);
println!("Arka Plan Rengi: {:?}", palette.background);
```
