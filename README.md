# Notepad

Ein simpler Texteditor in Rust, gebaut mit [egui/eframe](https://github.com/emilk/egui) und [rfd](https://github.com/PolyMeilex/rfd) für native Dateidialoge.

## Funktionen

- Neu, Öffnen, Speichern, Speichern unter
- Zeilennummern (auch bei Zeilenumbruch korrekt)
- Suchen und Ersetzen (Weitersuchen, Ersetzen, Alle ersetzen)
- Kodierungen: UTF-8, UTF-8 mit BOM, UTF-16 LE/BE, ANSI (Windows-1252)
- Zeilenenden: CRLF und LF werden erkannt und beim Speichern beibehalten
- Warnung vor dem Verwerfen ungespeicherter Änderungen

## Tastenkürzel

| Kürzel         | Aktion               |
|----------------|----------------------|
| Ctrl+N         | Neu                  |
| Ctrl+O         | Öffnen               |
| Ctrl+S         | Speichern            |
| Ctrl+F, Ctrl+H | Suchen/Ersetzen      |
| F3             | Weitersuchen         |
| Esc            | Suchleiste schließen |

## Bauen und starten

Voraussetzung: [Rust](https://rustup.rs) (Edition 2024).

```sh
cargo run --release
```

Das Binary liegt danach unter `target/release/notepad(.exe)`.

Tests:

```sh
cargo test
```

## Installer (Windows, .msi)

Das MSI installiert nur für den aktuellen Benutzer, ohne Admin-Rechte:

- `%LOCALAPPDATA%\Programs\Notepad\notepad.exe`
- Startmenü-Eintrag
- `.txt`-Zuordnung, „Neu → Textdokument“, Eintrag unter „Öffnen mit“

Deinstallation über „Apps & Features“ entfernt alles wieder.

Bauen (benötigt [WiX Toolset](https://wixtoolset.org) 7: `dotnet tool install --global wix`):

```powershell
cargo build -r
wix build wix\main.wxs -o target\notepad.msi
```

Als Standard-App muss der Notepad einmalig per „Öffnen mit → Andere App auswählen → Immer“
gewählt werden. Das lässt Windows auch Installern nicht zu.

Die MSI-Version wird aus `Cargo.toml` übernommen (über die Versionsinfo der exe).

## Icon

`assets/icon.ico` wird per `build.rs` in die exe eingebettet, `assets/icon.png` ist das Fenster-Icon.
Beide erzeugt `assets\make-icon.ps1 -OutDir assets` (danach `icon-16.png`/`icon-32.png` löschen,
das sind nur Vorschauen).

## Kodierung beim Öffnen

1. Datei hat ein BOM → UTF-8 mit BOM bzw. UTF-16
2. Gültiges UTF-8 → UTF-8
3. Sonst → ANSI (Windows-1252)

Die Kodierung zum Speichern lässt sich im Menü **Kodierung** ändern. Enthält der Text Zeichen, die in ANSI nicht darstellbar sind, wird angeboten, stattdessen als UTF-8 zu speichern.

## Einschränkungen

- Suche unterscheidet Groß-/Kleinschreibung.
- UTF-16 ohne BOM wird nicht erkannt.
