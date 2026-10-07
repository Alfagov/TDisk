use crate::scan_context::{ScanContext, SkipReason};
use rayon::prelude::*;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;

#[derive(Debug)]
pub struct FileNode {
    pub name: OsString,
    pub size: u64,
    pub children: Vec<FileNode>,
    pub is_dir: bool,
    pub pending: bool,
    pub errors: u64,
    pub skip_reason: Option<SkipReason>,
}

impl FileNode {
    pub fn new(name: OsString, size: u64, is_dir: bool) -> Self {
        Self {
            name,
            size,
            children: Vec::new(),
            is_dir,
            pending: is_dir,
            errors: 0,
            skip_reason: None,
        }
    }
}

#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub directories: AtomicU64,
    pub bytes: AtomicU64,
    pub root_sizes: OnceLock<Vec<AtomicU64>>,
}

#[derive(Debug)]
pub struct VolumeUsage {
    pub mount: PathBuf,
    pub used: u64,
    pub available: u64,
    pub total: u64,
}

pub enum ScanEvent {
    VolumeUsage(VolumeUsage),
    Started(FileNode),
    Directory { index: usize, node: FileNode },
    Finished,
    Failed(String),
}

/// Directory records stay in discovery order, so updates and navigation use stable indices.
/// Only directories are scheduled in Rayon; regular files need no additional I/O on macOS.
pub fn scan(path: PathBuf, tx: &Sender<ScanEvent>, progress: &Progress) {
    let setup = (|| -> io::Result<_> {
        let path = std::fs::canonicalize(&path)?;
        #[cfg(target_os = "macos")]
        if let Ok(usage) = crate::fs_mac::volume_usage(&path) {
            let _ = tx.send(ScanEvent::VolumeUsage(usage));
        }
        let context = ScanContext::new(&path)?;
        let root = read_node(&path, progress, &context, None)?;
        Ok((path, context, root))
    })();
    let (path, context, mut root) = match setup {
        Ok(result) => result,
        Err(error) => {
            let _ = tx.send(ScanEvent::Failed(format!("{}: {error}", path.display())));
            return;
        }
    };
    let _ = progress.root_sizes.set(
        root.children
            .iter()
            .map(|node| AtomicU64::new(node.size))
            .collect(),
    );
    let directories: Vec<_> = root
        .children
        .iter()
        .enumerate()
        .filter(|(_, node)| node.is_dir && node.skip_reason.is_none())
        .map(|(index, node)| (index, path.join(&node.name)))
        .collect();
    root.pending = true;
    if tx.send(ScanEvent::Started(root)).is_err() {
        return;
    }
    let scan_child = |(index, path): (usize, PathBuf)| {
        let node = scan_directory(
            &path,
            progress,
            &context,
            progress.root_sizes.get().and_then(|sizes| sizes.get(index)),
        );
        let _ = tx.send(ScanEvent::Directory { index, node });
    };
    if directories.len() <= 1 {
        directories.into_iter().for_each(scan_child);
    } else {
        directories.into_par_iter().for_each(scan_child);
    }
    let _ = tx.send(ScanEvent::Finished);
}

fn scan_directory(
    path: &Path,
    progress: &Progress,
    context: &ScanContext,
    branch: Option<&AtomicU64>,
) -> FileNode {
    let mut node = match read_node(path, progress, context, branch) {
        Ok(node) => node,
        Err(_) => {
            let mut node = FileNode::new(node_name(path), 0, true);
            node.errors = 1;
            node.pending = false;
            return node;
        }
    };
    // Enumeration has already released the fd and scratch-buffer borrow before recursion.
    let directories: Vec<_> = node
        .children
        .iter_mut()
        .filter(|child| child.is_dir)
        .collect();
    let scan_child = |child: &mut FileNode| {
        *child = scan_directory(&path.join(&child.name), progress, context, branch);
    };
    if directories.len() <= 1 {
        directories.into_iter().for_each(scan_child);
    } else {
        directories.into_par_iter().for_each(scan_child);
    }
    node.size = node.children.iter().map(|child| child.size).sum();
    node.errors += node.children.iter().map(|child| child.errors).sum::<u64>();
    node.pending = false;
    node
}

fn node_name(path: &Path) -> OsString {
    path.file_name().unwrap_or(path.as_os_str()).to_os_string()
}

fn read_node(
    path: &Path,
    progress: &Progress,
    context: &ScanContext,
    branch: Option<&AtomicU64>,
) -> io::Result<FileNode> {
    if let Some(reason) = context.directory_skip(path) {
        return Ok(skipped_directory(path, reason));
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "scan root is not a directory",
        ));
    }
    if context.is_other_volume(&metadata) {
        return Ok(skipped_directory(path, SkipReason::MountedVolume));
    }
    if !context.claim_directory(&metadata) {
        return Ok(skipped_directory(path, SkipReason::AlreadyCounted));
    }
    let (mut children, errors) = read_directory(path, Some(context))?;
    for child in children.iter_mut().filter(|node| node.is_dir) {
        if let Some(reason) = context.directory_skip(&path.join(&child.name)) {
            child.skip_reason = Some(reason);
            child.pending = false;
        }
    }
    let size = children
        .iter()
        .filter(|node| !node.is_dir)
        .map(|node| node.size)
        .sum();
    progress.files.fetch_add(
        children.iter().filter(|node| !node.is_dir).count() as u64,
        Ordering::Relaxed,
    );
    progress.directories.fetch_add(1, Ordering::Relaxed);
    progress.bytes.fetch_add(size, Ordering::Relaxed);
    if let Some(branch) = branch {
        branch.fetch_add(size, Ordering::Relaxed);
    }
    Ok(FileNode {
        name: node_name(path),
        size,
        children,
        is_dir: true,
        pending: true,
        errors,
        skip_reason: None,
    })
}

fn skipped_directory(path: &Path, reason: SkipReason) -> FileNode {
    let mut node = FileNode::new(node_name(path), 0, true);
    node.pending = false;
    node.skip_reason = Some(reason);
    node
}

#[cfg(target_os = "macos")]
fn read_directory(path: &Path, context: Option<&ScanContext>) -> io::Result<(Vec<FileNode>, u64)> {
    match crate::fs_mac::read_directory(path, context) {
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(libc::ENOTSUP | libc::ENOSYS | libc::EINVAL)
            ) =>
        {
            read_directory_portable(path, context)
        }
        result => result,
    }
}

#[cfg(not(target_os = "macos"))]
fn read_directory(path: &Path, context: Option<&ScanContext>) -> io::Result<(Vec<FileNode>, u64)> {
    read_directory_portable(path, context)
}

fn read_directory_portable(
    path: &Path,
    context: Option<&ScanContext>,
) -> io::Result<(Vec<FileNode>, u64)> {
    let mut children = Vec::new();
    let mut errors = 0;
    for entry in std::fs::read_dir(path)? {
        let result = (|| -> io::Result<()> {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                children.push(FileNode::new(entry.file_name(), 0, true));
            } else if kind.is_file() {
                let metadata = entry.metadata()?;
                #[cfg(unix)]
                let size = {
                    use std::os::unix::fs::MetadataExt;
                    metadata.blocks() * 512
                };
                #[cfg(not(unix))]
                let size = metadata.len();
                let mut node = FileNode::new(entry.file_name(), size, false);
                #[cfg(unix)]
                if let Some(context) = context {
                    use std::os::unix::fs::MetadataExt;
                    if !context.claim_file(
                        crate::scan_context::FileIdentity::from_metadata(&metadata),
                        metadata.nlink(),
                    ) {
                        node.size = 0;
                        node.skip_reason = Some(SkipReason::AlreadyCounted);
                    }
                }
                children.push(node);
            }
            Ok(())
        })();
        if result.is_err() {
            errors += 1;
        }
    }
    Ok((children, errors))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "tdisk-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn progressive_scan_matches_metadata_with_unicode_sparse_files_and_symlinks() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.0.join("nested")).unwrap();
        fs::create_dir(fixture.0.join("empty")).unwrap();
        fs::write(fixture.0.join("root-file"), [1; 8192]).unwrap();
        fs::write(fixture.0.join("nested/file"), [2; 4096]).unwrap();
        let sparse = fs::File::create(fixture.0.join("sparse")).unwrap();
        sparse.set_len(32 * 1024 * 1024).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let raw = OsString::from("日本語-é");
            fs::create_dir(fixture.0.join(&raw)).unwrap();
            fs::write(fixture.0.join(raw).join("file"), [3; 4096]).unwrap();
            symlink(&fixture.0, fixture.0.join("nested/cycle")).unwrap();
        }
        let expected = reference_size(&fixture.0);
        let progress = Progress::default();
        let (tx, rx) = mpsc::channel();
        scan(fixture.0.clone(), &tx, &progress);
        drop(tx);
        let mut events = rx
            .into_iter()
            .filter(|event| !matches!(event, ScanEvent::VolumeUsage(_)));
        let ScanEvent::Started(root) = events.next().unwrap() else {
            panic!("root must be published first")
        };
        assert!(root.pending);
        assert!(root.children.iter().filter(|n| n.is_dir).all(|n| n.pending));
        let mut app = crate::app::App::new(fixture.0.clone());
        app.apply(ScanEvent::Started(root));
        for event in events {
            app.apply(event);
        }
        app.refresh();
        assert!(!app.scanning);
        let root = app.root.unwrap();
        assert_eq!(root.size, expected);
        assert_eq!(root.errors, 0);
        assert!(root.children.iter().all(|n| !n.pending));
        assert_eq!(progress.bytes.load(Ordering::Relaxed), expected);
    }

    fn reference_size(path: &Path) -> u64 {
        fs::read_dir(path)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let kind = entry.file_type().unwrap();
                if kind.is_dir() {
                    reference_size(&entry.path())
                } else if kind.is_file() {
                    let metadata = entry.metadata().unwrap();
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::MetadataExt;
                        metadata.blocks() * 512
                    }
                    #[cfg(not(unix))]
                    {
                        metadata.len()
                    }
                } else {
                    0
                }
            })
            .sum()
    }

    #[test]
    fn bulk_scan_spans_multiple_batches() {
        let fixture = Fixture::new();
        for i in 0..2500 {
            fs::File::create(fixture.0.join(format!("{i:04}-{}", "x".repeat(80)))).unwrap();
        }
        let (children, errors) = read_directory(&fixture.0, None).unwrap();
        assert_eq!(children.len(), 2500);
        assert_eq!(errors, 0);
        let (portable, errors) = read_directory_portable(&fixture.0, None).unwrap();
        assert_eq!(portable.len(), children.len());
        assert_eq!(errors, 0);
    }

    #[cfg(unix)]
    #[test]
    fn hard_links_across_parallel_subtrees_are_counted_once_per_scan() {
        use std::os::unix::fs::MetadataExt;
        let fixture = Fixture::new();
        fs::create_dir(fixture.0.join("a")).unwrap();
        fs::create_dir(fixture.0.join("b")).unwrap();
        let original = fixture.0.join("a/file");
        fs::write(&original, [1; 8192]).unwrap();
        fs::hard_link(&original, fixture.0.join("b/link")).unwrap();
        let expected = fs::metadata(&original).unwrap().blocks() * 512;
        for with_root_link in [false, true] {
            if with_root_link {
                fs::hard_link(&original, fixture.0.join("root-link")).unwrap();
            }
            for _ in 0..3 {
                let progress = Progress::default();
                let (tx, rx) = mpsc::channel();
                scan(fixture.0.clone(), &tx, &progress);
                drop(tx);
                let mut app = crate::app::App::new(fixture.0.clone());
                for event in rx {
                    app.apply(event);
                }
                let root = app.root.unwrap();
                assert_eq!(root.size, expected);
                assert_eq!(root.errors, 0);
                assert_eq!(progress.bytes.load(Ordering::Relaxed), expected);
                let duplicates = root
                    .children
                    .iter()
                    .filter(|n| n.skip_reason.is_some())
                    .count()
                    + root
                        .children
                        .iter()
                        .flat_map(|n| &n.children)
                        .filter(|n| n.skip_reason.is_some())
                        .count();
                assert_eq!(duplicates, if with_root_link { 2 } else { 1 });
            }
        }
        let context = ScanContext::new(&fixture.0).unwrap();
        let (root, _) = read_directory_portable(&fixture.0, Some(&context)).unwrap();
        let (a, _) = read_directory_portable(&fixture.0.join("a"), Some(&context)).unwrap();
        let (b, _) = read_directory_portable(&fixture.0.join("b"), Some(&context)).unwrap();
        assert_eq!(
            root.iter().chain(&a).chain(&b).map(|n| n.size).sum::<u64>(),
            expected
        );
    }

    #[cfg(unix)]
    #[test]
    fn repeated_directory_identity_is_not_enumerated_twice() {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("file"), [1; 4096]).unwrap();
        let context = ScanContext::new(&fixture.0).unwrap();
        let progress = Progress::default();
        let first = read_node(&fixture.0, &progress, &context, None).unwrap();
        let second = read_node(&fixture.0, &progress, &context, None).unwrap();
        assert!(first.size > 0);
        assert_eq!(second.size, 0);
        assert_eq!(second.skip_reason, Some(SkipReason::AlreadyCounted));
        assert!(second.children.is_empty());
        assert!(!second.pending);
        assert_eq!(progress.directories.load(Ordering::Relaxed), 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn actual_macos_backing_mount_is_excluded_without_traversal() {
        let backing = Path::new("/System/Volumes/Data");
        if !crate::fs_mac::mount_points()
            .unwrap()
            .contains(&backing.to_path_buf())
        {
            return;
        }
        let context = ScanContext::new(Path::new("/")).unwrap();
        let progress = Progress::default();
        let node = read_node(backing, &progress, &context, None).unwrap();
        assert_eq!(node.skip_reason, Some(SkipReason::MountedVolume));
        assert_eq!(node.errors, 0);
        assert_eq!(node.size, 0);
        assert!(node.children.is_empty());
        assert_eq!(progress.directories.load(Ordering::Relaxed), 0);
        let users = fs::metadata("/Users").unwrap();
        let alias = fs::metadata("/System/Volumes/Data/Users").unwrap();
        assert!(context.claim_directory(&users));
        assert!(!context.claim_directory(&alias));
        let usage = crate::fs_mac::volume_usage(Path::new("/")).unwrap();
        assert_eq!(usage.mount, backing);
        assert!(usage.used > 0 && usage.used <= usage.total);
    }

    #[test]
    fn invalid_root_is_an_error_and_closed_receiver_is_safe() {
        let fixture = Fixture::new();
        let (tx, rx) = mpsc::channel();
        scan(fixture.0.join("missing"), &tx, &Progress::default());
        assert!(
            rx.try_iter()
                .any(|event| matches!(event, ScanEvent::Failed(_)))
        );
        fs::write(fixture.0.join("file"), []).unwrap();
        scan(fixture.0.join("file"), &tx, &Progress::default());
        assert!(
            rx.try_iter()
                .any(|event| matches!(event, ScanEvent::Failed(_)))
        );
        drop(rx);
        scan(fixture.0.clone(), &tx, &Progress::default());
    }
}
