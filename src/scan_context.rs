use std::collections::HashSet;
use std::fs::Metadata;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
}

impl FileIdentity {
    #[cfg(unix)]
    pub fn from_metadata(metadata: &Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkipReason {
    MountedVolume,
    AlreadyCounted,
}

impl SkipReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::MountedVolume => "other volume",
            Self::AlreadyCounted => "counted elsewhere",
        }
    }
}

#[derive(Default)]
struct SeenObjects {
    // Separate shards avoid serializing parallel directory enumeration. Only directory
    // identities and multiply-linked files are retained, not every regular file.
    shards: [Mutex<HashSet<FileIdentity>>; 32],
}

impl SeenObjects {
    fn insert(&self, identity: FileIdentity) -> bool {
        let shard = (identity.inode ^ identity.device.rotate_left(13)) as usize % self.shards.len();
        self.shards[shard]
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(identity)
    }
}

pub struct ScanContext {
    root: PathBuf,
    excluded_mounts: HashSet<PathBuf>,
    allowed_devices: HashSet<u64>,
    directories: SeenObjects,
    hard_links: SeenObjects,
}

impl ScanContext {
    pub fn new(root: &Path) -> io::Result<Self> {
        #[cfg(target_os = "macos")]
        let mounts = crate::fs_mac::mount_points()?;
        #[cfg(not(target_os = "macos"))]
        let mounts = Vec::new();
        Ok(Self::with_mounts(root, mounts))
    }

    fn with_mounts(root: &Path, mounts: Vec<PathBuf>) -> Self {
        let excluded_mounts = mounts
            .into_iter()
            .filter(|mount| mount != root && mount.starts_with(root))
            .collect();
        let mut allowed_devices = HashSet::new();
        #[cfg(unix)]
        if let Ok(metadata) = std::fs::metadata(root) {
            allowed_devices.insert(FileIdentity::from_metadata(&metadata).device);
        }
        // Older APFS versions may expose separate device numbers for the two
        // members of the startup volume group. Its user-facing firmlinks are valid.
        #[cfg(target_os = "macos")]
        if root == Path::new("/")
            && let Ok(metadata) = std::fs::metadata("/System/Volumes/Data")
        {
            allowed_devices.insert(FileIdentity::from_metadata(&metadata).device);
        }
        Self {
            root: root.to_path_buf(),
            excluded_mounts,
            allowed_devices,
            directories: SeenObjects::default(),
            hard_links: SeenObjects::default(),
        }
    }

    pub fn directory_skip(&self, path: &Path) -> Option<SkipReason> {
        if path != self.root && self.excluded_mounts.contains(path) {
            Some(SkipReason::MountedVolume)
        } else {
            None
        }
    }

    pub fn is_other_volume(&self, metadata: &Metadata) -> bool {
        #[cfg(unix)]
        {
            !self.allowed_devices.is_empty()
                && !self
                    .allowed_devices
                    .contains(&FileIdentity::from_metadata(metadata).device)
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            false
        }
    }

    pub fn claim_directory(&self, metadata: &Metadata) -> bool {
        #[cfg(unix)]
        {
            self.directories
                .insert(FileIdentity::from_metadata(metadata))
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            true
        }
    }

    pub fn claim_file(&self, identity: FileIdentity, links: u64) -> bool {
        links <= 1 || self.hard_links.insert(identity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_scan_excludes_backing_and_external_mounts_but_keeps_logical_users() {
        let mounts = vec![
            "/",
            "/System/Volumes/Data",
            "/System/Volumes/VM",
            "/Volumes/External",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect();
        let context = ScanContext::with_mounts(Path::new("/"), mounts);
        assert_eq!(
            context.directory_skip(Path::new("/System/Volumes/Data")),
            Some(SkipReason::MountedVolume)
        );
        assert_eq!(
            context.directory_skip(Path::new("/Volumes/External")),
            Some(SkipReason::MountedVolume)
        );
        assert_eq!(context.directory_skip(Path::new("/Users")), None);
        assert_eq!(context.directory_skip(Path::new("/Library")), None);
        assert_eq!(context.directory_skip(Path::new("/")), None);
    }

    #[test]
    fn explicitly_selected_volume_is_included_and_path_prefixes_are_component_based() {
        let root = Path::new("/System/Volumes/Data");
        let context = ScanContext::with_mounts(
            root,
            vec![
                root.to_path_buf(),
                root.join("mounted"),
                "/System/Volumes/Database".into(),
            ],
        );
        assert_eq!(context.directory_skip(root), None);
        assert_eq!(context.directory_skip(&root.join("Users")), None);
        assert_eq!(
            context.directory_skip(&root.join("mounted")),
            Some(SkipReason::MountedVolume)
        );
        assert_eq!(
            context.directory_skip(Path::new("/System/Volumes/Database")),
            None
        );
    }

    #[test]
    fn concurrent_claims_count_hard_links_once_and_do_not_merge_different_devices() {
        let context = ScanContext::with_mounts(Path::new("/"), Vec::new());
        let id = FileIdentity {
            device: 1,
            inode: 42,
        };
        let wins: usize = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..16)
                .map(|_| scope.spawn(|| usize::from(context.claim_file(id, 2))))
                .collect();
            threads
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .sum()
        });
        assert_eq!(wins, 1);
        assert!(context.claim_file(FileIdentity { device: 2, ..id }, 2));
        assert!(context.claim_file(
            FileIdentity {
                device: 1,
                inode: 100
            },
            1
        ));
        assert!(context.claim_file(
            FileIdentity {
                device: 1,
                inode: 100
            },
            1
        ));
    }
}
