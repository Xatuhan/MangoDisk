# Güvenlik Hardening Notları (`hardened-local` dalı)

> **Durum (2026-10-25):** Bu sürüm güvenlik açısından geliştirildi. Kod yazıldı ve
> bu dala push'landı, ancak **derlenmedi ve test edilmedi**. Disk alanı sıkıntısı
> nedeniyle derleme ortamı (Rust + Visual Studio C++ iş yükü, ilk derlemede ~600
> paket) kurulmadı. İleride istenirse aşağıdaki adımlarla derlenecek.

Temel: `main` = upstream `harry0703/MangoDisk` `b9199fd`. Denetim raporu ve
gerekçeler Obsidian'da: `MangoDisk Fork\FAZ 1.md` (denetim), `FAZ 2.md` (bu dal).

## Yapılan geliştirmeler

### 1. Ağ izolasyonu (dışarı veri çıkışı kökten kesildi)

1. **Otomatik güncelleme ve telemetri kapatıldı.** Açılıştan 3 sn sonra ve her 6
   saatte bir `mangodisk.app`'e giden; kalıcı kurulum kimliği, OS sürümü, sistem
   dili ve işlemci mimarisini gönderen kontrol döngüsü kaldırıldı. Kontrol artık ağa
   çıkmadan "güncel" döner. Telemetri başlıklarını üreten iki dosya silindi,
   arayüzün güncelleyiciyi doğrudan çağırma izni (`updater:default`) kaldırıldı.
   `src-tauri/src/services/app_updates.rs`, `src-tauri/capabilities/default.json`
2. **AI varsayılan olarak kapalı.** Önceden varsayılan açıktı ve "Açıkla"ya
   basınca dosya yollarını (kullanıcı adı dahil) sunucuya gönderiyordu.
   `mangodisk-core/src/ai/preferences.rs`
3. **AI'ın tüm ağ çıkışları engellendi.** Resmî servis (kota, açıklama, puanlama)
   ve kullanıcı tanımlı özel sağlayıcı; ayrıca ortak HTTP istemcisi kurulmadan
   `Disabled` hatası döner. Yanlışlıkla tıklansa bile paket çıkmaz.
   `mangodisk-core/src/ai/{mod,official,feedback,transport}.rs`
4. **Geri bildirim gönderimi kapatıldı.** Mesaj, ekler ve tanılama günlüğü
   arşivi (yolları ve kullanıcı adını içerir) hazırlanmaz, gönderilmez.
   `src-tauri/src/services/feedback.rs`

### 2. Yerel yetki yükseltme (LPE) ve komut yamaları

5. **Başlangıç öğeleri yardımcısına bütünlük kontrolü.** Yönetici yetkisiyle
   çalışan yardımcı, isteğini kullanıcının `%TEMP%` klasöründen okuyordu; araya
   giren bir süreç dosyayı değiştirip başka bir başlangıç öğesini sildirebilirdi.
   Artık istemcinin yazdığı baytların özeti (BLAKE3, sistem ayarları yardımcısıyla
   aynı mekanizma) yükseltme aracısı üzerinden gidiyor; dosya bir kez okunup
   doğrulanıyor, uymazsa hiçbir değişiklik yapılmıyor. Protokol v5 (eski sürümlerle
   karışmaz), macOS betiği de güncellendi, 2 yeni test eklendi.
   `mangodisk-platform/src/startup_helper.rs`, `elevation/protocol.rs`,
   `macos/privileged_startup_change.applescript`
6. **Görev çubuğu DLL'i kilitli yükleniyor.** Önbellekteki DLL içerik kontrolüyle
   yükleme arasında değiştirilebiliyordu. Artık dosya yazma/yeniden adlandırma/silmeye
   kapalı bir tanıtıcıyla doğrulanıyor ve `LoadLibraryW` bitene kadar kilitli
   tutuluyor; bağlantı noktası (junction/symlink) reddediliyor.
   `src-tauri/src/resident/taskbar_display/reservation_xaml.rs`
7. **PowerShell tam yolla çağrılıyor.** Çıplak `powershell.exe` yerine Windows'tan
   alınan `System32\WindowsPowerShell\v1.0\powershell.exe`; kurulum klasörüne
   bırakılan sahte bir `powershell.exe` çalıştırılamaz.
   `src-tauri/src/services/application_icon/windows.rs`

### 3. Çekirdek sürücüsü engeli

8. **PawnIO sürücüsü ve LocalSystem servisi kurulamaz.** CPU sıcaklığı için
   çekirdek (Ring-0) sürücüsü ve sistem servisi kuran işlem hem uygulamada (UAC
   sorulmadan önce) hem yardımcı exe'de reddedilir. Daha önce kurulmuşsa kaldırma
   hâlâ çalışır. `src-tauri/src/services/windows_monitoring.rs`,
   `mangodisk-privileged/src/windows/deployment.rs`

## Bilinen sınırlar

- Sıcaklık özelliği arayüzde "donanım desteklenmiyor" diye görünür; doğru metin
  ("bu derlemede kapatıldı") arayüz/çeviri değişikliği ister, yapılmadı.
- AI'ı daha önce açmış birinin kayıtlı tercihi açık kalır; ağ engeli yine geçerli.
- AI ağ engeli yalnızca `mangodisk-core`'un kendi birim testlerinde kalkar
  (yerel sahte sunucu testleri için); uygulama derlemelerinde her zaman açıktır.
- DLL kilidinde önbelleğin üst klasörleri bağlantı noktasına karşı denetlenmiyor.

## İleride derlemek istersek

1. Kurulum: `winget install Rustlang.Rustup` ve Visual Studio Installer'da
   "C++ ile masaüstü geliştirme" iş yükü (MSVC + Windows SDK). Node 22 ve pnpm var.
2. Doğrulama:
   ```
   pnpm install --frozen-lockfile
   node scripts/build-privileged.mjs
   cargo check --workspace --manifest-path src-tauri/Cargo.toml
   cargo test --manifest-path src-tauri/Cargo.toml -p mangodisk-core -p mangodisk-platform -p mangodisk-privileged
   cargo fmt --all -- --check
   ```
3. Çalıştırılabilir dosya: `pnpm tauri build --debug`. Release derleme, yönetici
   yardımcısının Authenticode ile imzalanmasını zorunlu tutar
   (`scripts/build-privileged.mjs`); imza sertifikası olmadığı için debug derleme
   kullanılmalı.
4. Kod hiç derlenmediği için ilk denemede derleme hatası çıkabilir; düzeltilip
   testler geçtikten sonra kullanılmalı.

O zamana kadar pratik alternatif: resmî sürüm + `MangoDisk.exe` için Windows
Güvenlik Duvarı giden trafik engeli (yalnızca ağ izolasyonunu sağlar, 5-8
numaralı yamaları sağlamaz).
