//! Crash-safe file writing.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Write to a temp file next to the target, then rename over it, so a crash or
/// power loss mid-save never leaves a truncated file.
// ponytail: rename replaces symlinks/hardlinks with a plain file and the file gets
// the folder's default permissions; fine for a notepad.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".notepad-tmp");
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_save_overwrites_and_cleans_up() {
        let dir = std::env::temp_dir().join(format!("notepad-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        write_atomic(&file, b"first").unwrap();
        write_atomic(&file, b"second").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"second");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "temp file left behind");
        // failure (target is a directory) must not leave a temp file either
        assert!(write_atomic(&dir, b"x").is_err());
        assert!(!dir.with_extension("notepad-tmp").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
