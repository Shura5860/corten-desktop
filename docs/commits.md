# Commit Mesajı Kuralları (Conventional Commits)

Corten projelerinde tüm Git commit mesajları **Conventional Commits** standartlarına uygun olmalıdır.

Biçim:
```text
<tip>(<kapsam>): <kısa açıklama>

[isteğe bağlı gövde]

[isteğe bağlı alt bilgi/kapanan issue'lar]
```

## Geçerli Tipler
- `feat`: Yeni bir özellik ekleme (ör. `feat(launcher): ek mod desteği`)
- `fix`: Hata düzeltme (ör. `fix(panel): ses seviyesi yüzdesi düzeltildi`)
- `refactor`: Kod yapısını iyileştirme (özellik veya hata değişikliği olmadan)
- `style`: Kod formatı, noktalama (`rustfmt` vb.)
- `perf`: Performans artırıcı değişiklik
- `test`: Birim veya entegrasyon testi ekleme/güncelleme
- `docs`: Dokümantasyon veya README güncellemeleri
- `chore`: Bağımlılık güncellemeleri, CI iş akışı ayarları

## Kurallar
1. İlk satır emir kipiyle ve küçük harfle başlar (İngilizce tercih edilir: `feat(theme): add dark mode tokens`).
2. İlk satır 72 karakteri geçmemelidir.
3. Nokta ile bitirilmez.
