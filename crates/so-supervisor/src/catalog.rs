//! Shared model catalog cache for handlers and run usage attribution.

use std::sync::Arc;

use so_router::Catalog;

/// The catalog, re-read only when `models.toml` changes on disk.
pub(crate) fn cached_catalog(paths: &so_core::Paths) -> Arc<Catalog> {
    type Slot = Option<(
        std::path::PathBuf,
        Option<std::time::SystemTime>,
        Arc<Catalog>,
    )>;
    static CACHE: std::sync::Mutex<Slot> = std::sync::Mutex::new(None);
    let file = paths.models_toml();
    let mtime = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
    let mut slot = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((path, seen, catalog)) = slot.as_ref()
        && *path == file
        && *seen == mtime
    {
        return catalog.clone();
    }
    let catalog = Arc::new(Catalog::load(paths).unwrap_or_else(|error| {
        tracing::warn!(%error, path = %file.display(), "could not load model catalog; using default catalog");
        Catalog::default_catalog()
    }));
    *slot = Some((file, mtime, catalog.clone()));
    catalog
}

#[cfg(test)]
mod tests {
    use super::*;
    use so_core::{Harness, Paths};

    #[test]
    fn catalog_recovers_when_a_bad_file_is_removed_and_then_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::from_dir(dir.path().to_path_buf());
        std::fs::write(paths.models_toml(), "invalid = [").unwrap();
        assert_eq!(
            cached_catalog(&paths).pool_for(Harness::Agy, "claude-sonnet-4-6"),
            "claude"
        );
        assert_eq!(
            cached_catalog(&paths).pool_for(Harness::Codex, "gpt-6-luna"),
            "default"
        );
        std::fs::remove_file(paths.models_toml()).unwrap();
        assert_eq!(*cached_catalog(&paths), Catalog::default_catalog());
        std::fs::write(
            paths.models_toml(),
            r#"[[agy.models]]
id = "custom-model"
tier = "high"
cost = "paid"
pool = "gemini"
"#,
        )
        .unwrap();
        assert_eq!(
            cached_catalog(&paths).pool_for(Harness::Agy, "custom-model"),
            "gemini"
        );
    }
}
