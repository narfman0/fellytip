//! `--incremental` skip logic — avoid regenerating entities whose atlas is
//! already newer than the bestiary source.

use std::path::Path;
use std::time::SystemTime;

/// Returns `true` iff `output_png` exists and its mtime is strictly newer
/// than `bestiary_toml`'s mtime.
pub fn can_skip(bestiary_toml: &Path, output_png: &Path) -> bool {
    let Ok(bm) = mtime(bestiary_toml) else { return false; };
    let Ok(om) = mtime(output_png) else { return false; };
    om > bm
}

fn mtime(p: &Path) -> std::io::Result<SystemTime> {
    std::fs::metadata(p)?.modified()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::thread;
    use std::time::Duration;
    use tempfile::tempdir;

    fn touch(path: &Path, content: &str) {
        let mut f = fs::File::create(path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn missing_output_is_not_skipped() {
        let dir = tempdir().unwrap();
        let b = dir.path().join("bestiary.toml");
        let o = dir.path().join("out.png");
        touch(&b, "");
        assert!(!can_skip(&b, &o));
    }

    #[test]
    fn older_output_is_not_skipped() {
        let dir = tempdir().unwrap();
        let b = dir.path().join("bestiary.toml");
        let o = dir.path().join("out.png");
        touch(&o, "");
        thread::sleep(Duration::from_millis(20));
        touch(&b, "");
        assert!(!can_skip(&b, &o));
    }

    #[test]
    fn newer_output_is_skipped() {
        let dir = tempdir().unwrap();
        let b = dir.path().join("bestiary.toml");
        let o = dir.path().join("out.png");
        touch(&b, "");
        thread::sleep(Duration::from_millis(20));
        touch(&o, "");
        assert!(can_skip(&b, &o));
    }
}
