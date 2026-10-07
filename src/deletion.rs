//! Recoverable deletion only. There is deliberately no permanent-delete fallback.
use crate::scan_context::FileIdentity;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct TrashCandidate {
    pub path: PathBuf,
    pub size: u64,
    pub incomplete: bool,
    root: PathBuf,
    identity: FileIdentity,
}

// The folder containers are protected; ordinary contents of Documents/Downloads/etc.
// remain eligible. System and application-support trees are protected recursively.
const SYSTEM_TREES: &[&str] = &[
    "/System",
    "/Library",
    "/usr",
    "/bin",
    "/sbin",
    "/dev",
    "/etc",
    "/var",
    "/private/etc",
    "/private/var",
];
const ROOT_CONTAINERS: &[&str] = &[
    "/",
    "/Users",
    "/Applications",
    "/Volumes",
    "/private",
    "/tmp",
    "/private/tmp",
    "/opt",
    "/cores",
    "/.vol",
    "/.Trash",
    "/.Trashes",
];
const HOME_CONTAINERS: &[&str] = &[
    "Desktop",
    "Documents",
    "Downloads",
    "Pictures",
    "Music",
    "Movies",
    "Public",
    "Applications",
];

fn logical_path(path: &Path) -> PathBuf {
    // Conservative protection also prevents mixed-case spellings from bypassing
    // the rules on case-insensitive macOS volumes. This does not change the I/O path.
    let path = PathBuf::from(path.to_string_lossy().to_ascii_lowercase());
    // The APFS Data mount exposes physical aliases for the startup directory tree.
    if let Ok(tail) = path.strip_prefix("/system/volumes/data") {
        Path::new("/").join(tail)
    } else {
        path
    }
}

fn protection_reason(
    path: &Path,
    root: &Path,
    home: Option<&Path>,
    mounts: &[PathBuf],
) -> Option<&'static str> {
    let path = logical_path(path);
    if logical_path(root).starts_with(&path) {
        return Some("The scan root and its parents cannot be moved to Trash.");
    }
    if mounts
        .iter()
        .any(|mount| logical_path(mount).starts_with(&path))
    {
        return Some("Volume roots and folders containing mounted volumes are protected.");
    }
    if ROOT_CONTAINERS
        .iter()
        .any(|item| path == logical_path(Path::new(item)))
        || SYSTEM_TREES
            .iter()
            .any(|item| path.starts_with(logical_path(Path::new(item))))
    {
        return Some("This system location is protected by TDisk.");
    }
    if path
        .components()
        .any(|component| matches!(component.as_os_str().to_str(), Some(".trash" | ".trashes")))
    {
        return Some("Trash folders and their contents are protected by TDisk.");
    }
    let mut homes = Vec::new();
    if let Some(home) = home {
        homes.push(logical_path(home));
    }
    // Protect other users' home containers too, even when running with elevated access.
    if let Ok(tail) = path.strip_prefix("/users")
        && let Some(user) = tail.components().next()
    {
        homes.push(Path::new("/users").join(user));
    }
    for home in homes {
        if home.starts_with(&path)
            || HOME_CONTAINERS
                .iter()
                .any(|name| path == home.join(name.to_ascii_lowercase()))
        {
            return Some(
                "Home folders and standard user folders are protected. Select an ordinary item inside instead.",
            );
        }
        if path.starts_with(home.join("library")) {
            return Some("User Library and Trash contents are protected by TDisk.");
        }
    }
    None
}

#[cfg(target_os = "macos")]
pub fn prepare(
    path: &Path,
    root: &Path,
    size: u64,
    incomplete: bool,
) -> Result<TrashCandidate, String> {
    // Obtain a fresh mount inventory at both confirmation and execution. Fail closed.
    let mounts = crate::fs_mac::mount_points().map_err(|error| error.to_string())?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    prepare_with(path, root, size, incomplete, home.as_deref(), &mounts)
}

#[cfg(not(target_os = "macos"))]
pub fn prepare(_: &Path, _: &Path, _: u64, _: bool) -> Result<TrashCandidate, String> {
    Err("Move to Trash is currently available only on macOS. Nothing was deleted.".into())
}

fn prepare_with(
    path: &Path,
    root: &Path,
    size: u64,
    incomplete: bool,
    home: Option<&Path>,
    mounts: &[PathBuf],
) -> Result<TrashCandidate, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !(metadata.is_file() || metadata.is_dir()) {
        return Err(
            "Only regular files and folders can be moved to Trash; symbolic links are excluded."
                .into(),
        );
    }
    let path = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    let root = std::fs::canonicalize(root).map_err(|error| error.to_string())?;
    if path == root || !path.starts_with(&root) {
        return Err("The selected item must be inside the scan root.".into());
    }
    if let Some(reason) = protection_reason(&path, &root, home, mounts) {
        return Err(reason.into());
    }
    // Check identity again after resolving aliases. A symlink swap cannot silently
    // change the object shown in the confirmation into a different target.
    let resolved = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    let identity = FileIdentity::from_metadata(&metadata);
    if identity != FileIdentity::from_metadata(&resolved) || resolved.file_type().is_symlink() {
        return Err("The selected item changed. Refresh the scan and try again.".into());
    }
    Ok(TrashCandidate {
        path,
        root,
        identity,
        size,
        incomplete,
    })
}

impl TrashCandidate {
    pub fn move_to_trash(self) -> Result<(), String> {
        // Revalidate protection, mounts and object identity just before the OS call.
        let fresh = prepare(&self.path, &self.root, self.size, self.incomplete)?;
        self.validate_same_item(&fresh)?;
        trash_item(&self.path)
            .map_err(|error| format!("macOS could not complete Move to Trash. {error}"))
    }

    fn validate_same_item(&self, fresh: &Self) -> Result<(), String> {
        if self.path != fresh.path || self.identity != fresh.identity {
            Err("The selected item was replaced since confirmation. Nothing was deleted; refresh and try again.".into())
        } else {
            Ok(())
        }
    }
}

#[cfg(target_os = "macos")]
fn trash_item(path: &Path) -> Result<(), String> {
    use objc2::{
        class, msg_send,
        rc::{Retained, autoreleasepool},
        runtime::{AnyObject, Bool},
    };
    use std::ffi::{CStr, CString, c_char};
    use std::os::unix::ffi::OsStrExt;
    // Loads the Foundation classes; objc2 supplies checked message dispatch and ownership.
    #[link(name = "Foundation", kind = "framework")]
    unsafe extern "C" {}
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|error| error.to_string())?;
    autoreleasepool(|_| {
        // SAFETY: selectors and argument types match Foundation's NSURL/NSFileManager
        // APIs. Retained objects and the error's borrowed text stay inside the pool.
        unsafe {
            let url: Retained<AnyObject> = msg_send![class!(NSURL),
                fileURLWithFileSystemRepresentation: path.as_ptr(),
                isDirectory: Bool::NO,
                relativeToURL: std::ptr::null::<AnyObject>()];
            let manager: Retained<AnyObject> = msg_send![class!(NSFileManager), defaultManager];
            let mut error: *mut AnyObject = std::ptr::null_mut();
            let result: Bool = msg_send![&*manager, trashItemAtURL: &*url,
                resultingItemURL: std::ptr::null_mut::<*mut AnyObject>(), error: &mut error];
            if result.as_bool() {
                return Ok(());
            }
            if error.is_null() {
                return Err("macOS could not move this item to Trash.".into());
            }
            let description: Retained<AnyObject> = msg_send![error, localizedDescription];
            let utf8: *const c_char = msg_send![&*description, UTF8String];
            if utf8.is_null() {
                return Err("macOS could not move this item to Trash.".into());
            }
            Err(CStr::from_ptr(utf8).to_string_lossy().into_owned())
        }
    })
}

#[cfg(not(target_os = "macos"))]
fn trash_item(_: &Path) -> Result<(), String> {
    Err("Move to Trash is currently available only on macOS.".into())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(crate) struct Fixture(pub PathBuf);
    impl Fixture {
        pub(crate) fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = Path::new("/tmp").join(format!(
                "tdisk-trash-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn system_aliases_home_containers_and_mount_ancestors_are_protected() {
        let home = Path::new("/Users/alex");
        for item in [
            "/",
            "/Users",
            "/Users/alex",
            "/Users/other",
            "/Users/alex/Downloads",
            "/Users/other/Pictures",
            "/Users/alex/Library/Caches",
            "/System/Volumes/Data/Users/alex/Documents",
            "/System/Volumes/Data/Library",
            "/System/Library",
            "/usr/local",
            "/Applications",
            "/private/var/db",
            "/Volumes/Disk",
            "/work/with-mount",
            "/users/ALEx/downloads",
            "/SYSTEM/volumes/data/users/Alex/library/caches",
            "/Volumes/dISK",
            "/Volumes/Disk/.Trashes/501/item",
        ] {
            assert!(
                protection_reason(
                    Path::new(item),
                    Path::new("/"),
                    Some(home),
                    &["/Volumes/Disk".into(), "/work/with-mount/nested".into()]
                )
                .is_some(),
                "{item}"
            );
        }
        for item in [
            "/Users/alex/Downloads/old.zip",
            "/Users/alex/Documents/archive",
            "/Applications/Example.app",
            "/Users/other/Pictures/photo.jpg",
            "/Volumes/Disk/old",
            "/System/Volumes/Data/Users/alex/Downloads/old.zip",
            "/Users/alex/Library-old",
            "/work/with-mountain",
        ] {
            assert!(
                protection_reason(
                    Path::new(item),
                    Path::new("/"),
                    Some(home),
                    &["/Volumes/Disk".into(), "/work/with-mount/nested".into()]
                )
                .is_none(),
                "{item}"
            );
        }
    }

    #[test]
    fn replaced_objects_and_symlink_escapes_are_rejected() {
        let fixture = Fixture::new();
        let file = fixture.0.join("candidate");
        std::fs::write(&file, b"first").unwrap();
        let original = prepare_with(&file, &fixture.0, 5, false, None, &[]).unwrap();
        std::fs::rename(&file, fixture.0.join("original")).unwrap();
        std::fs::write(&file, b"replacement").unwrap();
        let fresh = prepare_with(&file, &fixture.0, 11, false, None, &[]).unwrap();
        assert!(original.validate_same_item(&fresh).is_err());
        std::os::unix::fs::symlink(&file, fixture.0.join("link")).unwrap();
        assert!(prepare_with(&fixture.0.join("link"), &fixture.0, 0, false, None, &[]).is_err());
        let inside = fixture.0.join("inside");
        std::fs::create_dir(&inside).unwrap();
        std::os::unix::fs::symlink(&fixture.0, inside.join("escape")).unwrap();
        assert!(
            prepare_with(
                &inside.join("escape/candidate"),
                &inside,
                0,
                false,
                None,
                &[]
            )
            .is_err()
        );
    }

    #[test]
    fn root_missing_files_and_nested_mounts_fail_closed() {
        let fixture = Fixture::new();
        let folder = fixture.0.join("folder");
        std::fs::create_dir(&folder).unwrap();
        assert!(prepare_with(&fixture.0, &fixture.0, 0, false, None, &[]).is_err());
        assert!(prepare_with(&fixture.0.join("missing"), &fixture.0, 0, false, None, &[]).is_err());
        assert!(
            prepare_with(
                &folder,
                &fixture.0,
                0,
                false,
                None,
                &[std::fs::canonicalize(&folder).unwrap().join("mounted")]
            )
            .is_err()
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn foundation_trash_failure_is_reported_without_fallback() {
        let fixture = Fixture::new();
        assert!(trash_item(&fixture.0.join("does-not-exist")).is_err());
    }

    #[test]
    #[cfg(target_os = "macos")]
    #[ignore = "Integration check moves only a newly generated test folder to macOS Trash"]
    fn native_trash_moves_only_the_confirmed_fixture() {
        let fixture = Fixture::new();
        let candidate = fixture.0.join("TDisk-disposable-trash-check");
        std::fs::create_dir(&candidate).unwrap();
        std::fs::write(
            candidate.join("test.txt"),
            b"Disposable TDisk integration fixture",
        )
        .unwrap();
        std::fs::write(fixture.0.join("keep.txt"), b"Keep this sibling").unwrap();
        let prepared = prepare(&candidate, &fixture.0, 0, false).unwrap();
        prepared.move_to_trash().unwrap();
        assert!(!candidate.exists());
        assert!(fixture.0.join("keep.txt").exists());
    }
}
