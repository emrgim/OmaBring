use std::fs;
use std::path::Path;

use crate::error::{Error, Result};
use crate::Db;

impl Db {
    /// Copy the sqlite database (and WAL companions when present) to `dest_dir`.
    pub fn backup_to(&self, dest_dir: impl AsRef<Path>) -> Result<std::path::PathBuf> {
        let dest_dir = dest_dir.as_ref();
        fs::create_dir_all(dest_dir)?;
        let dest = dest_dir.join("omabring.db");
        fs::copy(self.path(), &dest)?;
        for suffix in ["-wal", "-shm"] {
            let side = Path::new(&format!("{}{suffix}", self.path().display())).to_path_buf();
            if side.is_file() {
                let _ = fs::copy(&side, dest_dir.join(format!("omabring.db{suffix}")));
            }
        }
        let ts = crate::db::now_secs();
        self.set_last_backup_at(ts)?;
        Ok(dest)
    }

    pub fn backup_configured(&self) -> Result<std::path::PathBuf> {
        let (path, _, _) = self.backup_settings()?;
        if path.trim().is_empty() {
            return Err(Error::Invalid("backup path is not set".into()));
        }
        self.backup_to(path)
    }
}
