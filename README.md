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

| Kürzel         | Aktion            |
|----------------|-------------------|
| Ctrl+N         | Neu               |
| Ctrl+O         | Öffnen            |
| Ctrl+S         | Speichern         |
| Ctrl+F, Ctrl+H | Suchen/Ersetzen   |
| F3             | Weitersuchen      |
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

## Kodierung beim Öffnen

1. Datei hat ein BOM → UTF-8 mit BOM bzw. UTF-16
2. Gültiges UTF-8 → UTF-8
3. Sonst → ANSI (Windows-1252)

Die Kodierung zum Speichern lässt sich im Menü **Kodierung** ändern. Enthält der Text Zeichen, die in ANSI nicht darstellbar sind, wird angeboten, stattdessen als UTF-8 zu speichern.

## Einschränkungen

- Suche unterscheidet Groß-/Kleinschreibung.
- UTF-16 ohne BOM wird nicht erkannt.
