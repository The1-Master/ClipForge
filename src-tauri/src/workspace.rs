//! The local fork owns files only inside the explicitly authorized workspace.
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

pub const ROOT: &str = r"J:\CS2ClipStudio";
static ACTIVE_ROOT: OnceLock<PathBuf> = OnceLock::new();

fn ordinary_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    if let Some(text) = path.to_str() {
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            if let Some(unc) = rest.strip_prefix(r"UNC\") {
                return PathBuf::from(format!(r"\\{unc}"));
            }
            return PathBuf::from(rest);
        }
    }
    path
}

fn within_legacy(path: &Path, legacy: &Path) -> bool {
    #[cfg(windows)]
    {
        let path = ordinary_path(path.to_path_buf()).to_string_lossy().replace('/', "\\");
        let root = legacy.to_string_lossy().replace('/', "\\");
        let prefix = format!("{}\\", root.trim_end_matches('\\'));
        return path.get(..prefix.len()).is_some_and(|part|part.eq_ignore_ascii_case(&prefix));
    }
    #[cfg(not(windows))]
    path.starts_with(legacy)
}

fn normalized_root(path: PathBuf) -> PathBuf {
    std::fs::canonicalize(&path).map(ordinary_path).unwrap_or(path)
}

pub fn root() -> PathBuf {
    ACTIVE_ROOT.get_or_init(|| {
        if let Some(value) = std::env::var_os("CLIPFORGE_WORKSPACE") {
            let path = PathBuf::from(value);
            if path.is_absolute() && !path.components().any(|p|matches!(p,Component::ParentDir)) {
                return normalized_root(path);
            }
        }
        let exe = std::env::current_exe().ok().map(ordinary_path);
        if let Some(folder) = exe.as_ref().and_then(|path|path.parent()) {
            if folder.join("clipforge-portable.json").is_file() {
                return normalized_root(folder.to_path_buf());
            }
        }
        let legacy = PathBuf::from(ROOT);
        if exe.as_ref().is_some_and(|path|within_legacy(path, &legacy))
            || (exe.is_none() && legacy.is_dir()) {
            return normalized_root(legacy);
        }
        // Public portable/installer builds own their complete adjacent workspace.
        normalized_root(exe.and_then(|path|path.parent().map(Path::to_path_buf)).unwrap_or(legacy))
    }).clone()
}

/// Resolve the nearest existing ancestor as well as every existing link/junction.
/// Missing children remain allowed, but parent traversal and workspace-root writes do not.
pub fn owned_path(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute()
        || path.components().any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err("Choose an absolute folder inside the ClipForge workspace.".into());
    }
    let workspace = std::fs::canonicalize(root()).map_err(|e| e.to_string())?;
    let mut ancestor = path;
    let mut suffix = Vec::new();
    loop {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        suffix.push(ancestor.file_name().ok_or("Cannot resolve workspace path")?.to_os_string());
        ancestor = ancestor.parent().ok_or("Cannot resolve workspace path")?;
    }
    let mut resolved = std::fs::canonicalize(ancestor).map_err(|e| e.to_string())?;
    for part in suffix.iter().rev() {
        resolved.push(part);
    }
    if resolved == workspace || !resolved.starts_with(&workspace) {
        return Err("Project files must stay inside the ClipForge workspace.".into());
    }
    Ok(ordinary_path(resolved))
}

pub fn initialize_environment() -> Result<(), String> {
    std::fs::create_dir_all(root()).map_err(|e|e.to_string())?;
    std::env::set_var("CLIPFORGE_WORKSPACE", root());
    let temp = root().join("temp/clipforge");
    let appdata = root().join("cache/clipforge-appdata");
    let local = root().join("cache/clipforge-localappdata");
    let webview = root().join("cache/webview");
    for directory in [&temp, &appdata, &local, &webview] {
        owned_path(directory)?;
        std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    }
    // Set before the WebView/runtime starts threads. Owned children inherit these.
    std::env::set_var("TEMP", &temp);
    std::env::set_var("TMP", &temp);
    std::env::set_var("APPDATA", &appdata);
    std::env::set_var("LOCALAPPDATA", &local);
    std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", root().join("cache/webview"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_rejects_outside_relative_and_parent_traversal() {
        assert!(owned_path(Path::new(r"C:\Users\Public\clipforge")).is_err());
        assert!(owned_path(Path::new(r"relative\clips")).is_err());
        assert!(owned_path(Path::new(r"J:\CS2ClipStudio\data\..\clips")).is_err());
        assert!(owned_path(&root()).is_err());
        assert!(owned_path(&root().join("data/clipforge")).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn extended_windows_paths_preserve_private_workspace_and_media_paths() {
        let extended = ordinary_path(PathBuf::from(r"\\?\J:\CS2ClipStudio\artifacts\ClipForge\app.exe"));
        assert!(extended.starts_with(Path::new(ROOT)));
        assert!(within_legacy(Path::new(r"\\?\j:\cs2clipstudio\artifacts\clipforge\app.exe"),Path::new(ROOT)));
        assert!(!within_legacy(Path::new(r"J:\CS2ClipStudio-other\app.exe"),Path::new(ROOT)));
        assert_eq!(ordinary_path(PathBuf::from(r"\\?\UNC\server\share\file")), PathBuf::from(r"\\server\share\file"));
        let owned = owned_path(&root().join("data/clipforge")).unwrap();
        assert!(owned.starts_with(root()));
        assert!(!owned.to_string_lossy().starts_with(r"\\?\"));
    }
}
