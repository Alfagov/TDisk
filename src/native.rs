//! Small, ownership-explicit C bridge. The Rust tree stays here; Swift receives only
//! the current directory and incremental row updates, never the recursive tree.
use crate::{
    deletion,
    filesystem::{self, FileNode, Progress, ScanEvent, VolumeUsage},
};
use serde::Serialize;
use std::{
    collections::HashMap,
    ffi::{CStr, CString, OsString, c_char},
    os::unix::ffi::{OsStrExt, OsStringExt},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering, mpsc},
    time::Instant,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    id: String,
    name: String,
    path: String,
    size: u64,
    is_directory: bool,
    pending: bool,
    errors: u64,
    skip_reason: Option<&'static str>,
    trash_blocked_reason: Option<String>,
    actions_available: bool,
}
#[derive(Serialize)]
struct Volume {
    mount: String,
    used: u64,
    available: u64,
    total: u64,
}
impl From<&VolumeUsage> for Volume {
    fn from(v: &VolumeUsage) -> Self {
        Self {
            mount: v.mount.to_string_lossy().into(),
            used: v.used,
            available: v.available,
            total: v.total,
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    root_id: String,
    root_path: String,
    status: &'static str,
    error: Option<String>,
    volume: Option<Volume>,
    folder: Option<Row>,
    breadcrumbs: Vec<Row>,
    listing_revision: u64,
    children: Option<Vec<Row>>,
    updates: Vec<Row>,
    files: u64,
    directories: u64,
    elapsed_milliseconds: u64,
    totals_stale: bool,
}
struct Session {
    path: PathBuf,
    root: Option<FileNode>,
    volume: Option<VolumeUsage>,
    receiver: mpsc::Receiver<ScanEvent>,
    progress: Arc<Progress>,
    status: &'static str,
    error: Option<String>,
    revision: u64,
    root_revision: u64,
    branches: HashMap<OsString, u64>,
    home: Option<PathBuf>,
    mounts: Vec<PathBuf>,
    prepared: Option<(PathBuf, deletion::TrashCandidate)>,
    stale: bool,
    started: Instant,
    elapsed: u64,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.progress.cancelled.store(true, Ordering::Relaxed);
    }
}
fn encode_path(path: &Path) -> String {
    use std::fmt::Write;
    let mut id = String::with_capacity(path.as_os_str().len() * 2);
    for byte in path.as_os_str().as_bytes() {
        let _ = write!(id, "{byte:02x}");
    }
    id
}
fn decode_path(id: &str) -> Option<PathBuf> {
    if !id.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Option<Vec<_>> = id
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let a = (pair[0] as char).to_digit(16)?;
            let b = (pair[1] as char).to_digit(16)?;
            Some((a * 16 + b) as u8)
        })
        .collect();
    Some(PathBuf::from(OsString::from_vec(bytes?)))
}
fn node_at<'a>(root: &'a FileNode, path: &Path, base: &Path) -> Option<&'a FileNode> {
    let mut node = root;
    for name in path.strip_prefix(base).ok()?.components() {
        node = node
            .children
            .iter()
            .find(|child| child.name == name.as_os_str())?;
    }
    Some(node)
}
impl Session {
    fn new(path: PathBuf) -> Self {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let progress = Arc::new(Progress::default());
        let (tx, receiver) = mpsc::channel();
        let worker_path = path.clone();
        let worker_progress = Arc::clone(&progress);
        std::thread::spawn(move || {
            if catch_unwind(AssertUnwindSafe(|| {
                filesystem::scan(worker_path, &tx, &worker_progress)
            }))
            .is_err()
            {
                let _ = tx.send(ScanEvent::Failed(
                    "The scanner stopped unexpectedly. Retry this location.".into(),
                ));
            }
        });
        Self {
            path,
            root: None,
            volume: None,
            receiver,
            progress,
            status: "scanning",
            error: None,
            revision: 1,
            root_revision: 1,
            branches: HashMap::new(),
            home: std::env::var_os("HOME").and_then(|home| std::fs::canonicalize(home).ok()),
            mounts: crate::fs_mac::mount_points().unwrap_or_default(),
            prepared: None,
            stale: false,
            started: Instant::now(),
            elapsed: 0,
        }
    }
    fn drain(&mut self) {
        loop {
            match self.receiver.try_recv() {
                Ok(ScanEvent::VolumeUsage(v)) => self.volume = Some(v),
                Ok(ScanEvent::Started(root)) => {
                    self.root = Some(root);
                    self.revision += 1;
                    self.root_revision = self.revision;
                }
                Ok(ScanEvent::Directory { index, node }) => {
                    if let Some(root) = &mut self.root
                        && let Some(child) = root.children.get_mut(index)
                    {
                        self.revision += 1;
                        self.branches.insert(node.name.clone(), self.revision);
                        root.size = root
                            .size
                            .saturating_sub(child.size)
                            .saturating_add(node.size);
                        root.errors = root
                            .errors
                            .saturating_sub(child.errors)
                            .saturating_add(node.errors);
                        *child = node;
                    }
                }
                Ok(ScanEvent::Finished) => {
                    let stopped = self.progress.cancelled.load(Ordering::Relaxed);
                    self.status = if stopped { "stopped" } else { "complete" };
                    if let Some(root) = &mut self.root {
                        root.pending = stopped;
                    }
                    self.elapsed = self.started.elapsed().as_millis() as u64;
                }
                Ok(ScanEvent::Failed(message)) => {
                    let stopped = self.progress.cancelled.load(Ordering::Relaxed);
                    self.status = if stopped { "stopped" } else { "failed" };
                    if !stopped {
                        self.error = Some(message);
                    }
                    self.elapsed = self.started.elapsed().as_millis() as u64;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if self.status == "scanning" {
                        self.status = "failed";
                        self.error = Some("Scanner stopped before completion.".into());
                    }
                    break;
                }
            }
        }
        if let Some(root) = &mut self.root
            && let Some(sizes) = self.progress.root_sizes.get()
        {
            for (child, size) in root.children.iter_mut().zip(sizes) {
                if child.pending {
                    child.size = size.load(Ordering::Relaxed);
                }
            }
            // Only apply the old progress array before a deletion changes root indices.
            if !self.stale {
                root.size = root.children.iter().map(|child| child.size).sum();
            }
        }
    }
    fn row(&self, node: &FileNode, path: &Path) -> Row {
        let actions_available = path.to_str().is_some();
        let blocked = if !actions_available {
            Some("This filename can't be represented by the native file actions.".into())
        } else if node.pending {
            Some("Finish scanning this folder before moving it to Trash.".into())
        } else if node.skip_reason.is_some() {
            Some("Excluded entries cannot be moved to Trash from this scan.".into())
        } else {
            deletion::protection_reason(path, &self.path, self.home.as_deref(), &self.mounts)
                .map(str::to_owned)
        };
        Row {
            id: encode_path(path),
            name: node.name.to_string_lossy().into_owned(),
            path: path.to_string_lossy().into_owned(),
            size: node.size,
            is_directory: node.is_dir,
            pending: node.pending,
            errors: node.errors,
            skip_reason: node.skip_reason.map(|r| r.label()),
            trash_blocked_reason: blocked,
            actions_available,
        }
    }
    fn snapshot(&mut self, id: &str, previous: u64) -> Snapshot {
        self.drain();
        let path = if id.is_empty() {
            self.path.clone()
        } else {
            decode_path(id).unwrap_or_default()
        };
        let mut breadcrumbs = Vec::new();
        let mut folder = None;
        let mut children = None;
        let mut updates = Vec::new();
        let mut listing_revision = 0;
        if let Some(root) = &self.root {
            breadcrumbs.push(self.row(root, &self.path));
            if let Ok(tail) = path.strip_prefix(&self.path) {
                let mut parent = self.path.clone();
                for component in tail.components() {
                    parent.push(component);
                    if let Some(node) = node_at(root, &parent, &self.path) {
                        breadcrumbs.push(self.row(node, &parent));
                    } else {
                        break;
                    }
                }
                if let Some(node) = node_at(root, &path, &self.path) {
                    folder = Some(self.row(node, &path));
                    listing_revision = if path == self.path {
                        self.root_revision
                    } else {
                        tail.components()
                            .next()
                            .and_then(|c| self.branches.get(c.as_os_str()))
                            .copied()
                            .unwrap_or(1)
                    };
                    if previous != listing_revision {
                        children = Some(
                            node.children
                                .iter()
                                .map(|child| self.row(child, &path.join(&child.name)))
                                .collect(),
                        );
                    } else if path == self.path && !self.stale {
                        updates = node
                            .children
                            .iter()
                            .filter(|child| child.is_dir)
                            .map(|child| self.row(child, &path.join(&child.name)))
                            .collect();
                    }
                }
            }
        }
        Snapshot {
            root_id: encode_path(&self.path),
            root_path: self.path.to_string_lossy().into_owned(),
            status: self.status,
            error: self.error.clone(),
            volume: self.volume.as_ref().map(Volume::from),
            folder,
            breadcrumbs,
            listing_revision,
            children,
            updates,
            files: self.progress.files.load(Ordering::Relaxed),
            directories: self.progress.directories.load(Ordering::Relaxed),
            elapsed_milliseconds: if self.status == "scanning" {
                self.started.elapsed().as_millis() as u64
            } else {
                self.elapsed
            },
            totals_stale: self.stale,
        }
    }
    fn prepare_trash(&mut self, id: &str) -> serde_json::Value {
        self.drain();
        self.prepared = None;
        let result = (|| -> Result<_, String> {
            if self.status == "scanning" {
                return Err("Wait for scanning to finish before moving an item to Trash.".into());
            }
            let path = decode_path(id).ok_or("Invalid item identifier")?;
            let node = self
                .root
                .as_ref()
                .and_then(|root| node_at(root, &path, &self.path))
                .ok_or("This item is no longer in the scan. Rescan and try again.")?;
            let row = self.row(node, &path);
            if let Some(reason) = row.trash_blocked_reason {
                return Err(reason);
            }
            let candidate =
                deletion::prepare(&path, &self.path, node.size, node.errors > 0 || self.stale)?;
            let reply = serde_json::json!({"ok":true, "id":id, "name":row.name, "path":candidate.path.to_string_lossy(), "size":candidate.size, "incomplete":candidate.incomplete, "isDirectory":node.is_dir});
            self.prepared = Some((path, candidate));
            Ok(reply)
        })();
        result.unwrap_or_else(|error| serde_json::json!({"ok":false,"error":error}))
    }
    fn trash(&mut self) -> serde_json::Value {
        let Some((path, candidate)) = self.prepared.take() else {
            return serde_json::json!({"ok":false,"error":"No confirmed item is pending."});
        };
        match candidate.move_to_trash() {
            Ok(()) => {
                fn remove(node: &mut FileNode, components: &[OsString]) -> Option<(u64, u64)> {
                    let (name, tail) = components.split_first()?;
                    let index = node.children.iter().position(|child| &child.name == name)?;
                    let removed = if tail.is_empty() {
                        let removed = node.children.remove(index);
                        (removed.size, removed.errors)
                    } else {
                        remove(&mut node.children[index], tail)?
                    };
                    node.size = node.size.saturating_sub(removed.0);
                    node.errors = node.errors.saturating_sub(removed.1);
                    Some(removed)
                }
                if let Some(root) = &mut self.root
                    && let Ok(tail) = path.strip_prefix(&self.path)
                {
                    let components: Vec<_> = tail
                        .components()
                        .map(|c| c.as_os_str().to_owned())
                        .collect();
                    remove(root, &components);
                    self.revision += 1;
                    self.root_revision = self.revision;
                    if let Some(first) = components.first() {
                        self.branches.insert(first.clone(), self.revision);
                    }
                }
                self.stale = true;
                self.volume = crate::fs_mac::volume_usage(&self.path).ok();
                serde_json::json!({"ok":true})
            }
            Err(error) => serde_json::json!({"ok":false,"error":error}),
        }
    }
    fn issues(&mut self) -> serde_json::Value {
        self.drain();
        fn walk(
            node: &FileNode,
            path: &Path,
            list: &mut Vec<serde_json::Value>,
            omitted: &mut bool,
        ) {
            let own = node
                .errors
                .saturating_sub(node.children.iter().map(|child| child.errors).sum());
            if own > 0 {
                if list.len() < 500 {
                    list.push(serde_json::json!({"path":path.to_string_lossy(),"count":own}));
                } else {
                    *omitted = true;
                }
            }
            for child in node
                .children
                .iter()
                .filter(|child| child.is_dir && child.errors > 0)
            {
                walk(child, &path.join(&child.name), list, omitted);
            }
        }
        let mut locations = Vec::new();
        let mut omitted = false;
        if let Some(root) = &self.root {
            walk(root, &self.path, &mut locations, &mut omitted);
        }
        serde_json::json!({"locations":locations,"omitted":omitted})
    }
}
fn json_output<T: Serialize>(f: impl FnOnce() -> T) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        serde_json::to_string(&f())
            .ok()
            .and_then(|json| CString::new(json).ok())
            .map_or(std::ptr::null_mut(), CString::into_raw)
    }))
    .unwrap_or(std::ptr::null_mut())
}
unsafe fn input<'a>(value: *const c_char) -> Option<&'a str> {
    if value.is_null() {
        None
    } else {
        unsafe { CStr::from_ptr(value) }.to_str().ok()
    }
}
/// # Safety
/// `path` must point to a valid, NUL-terminated UTF-8 path for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tdisk_scan_create(path: *const c_char) -> *mut std::ffi::c_void {
    catch_unwind(AssertUnwindSafe(|| {
        unsafe { input(path) }
            .map(|path| Box::into_raw(Box::new(Session::new(path.into()))).cast())
            .unwrap_or(std::ptr::null_mut())
    }))
    .unwrap_or(std::ptr::null_mut())
}
/// # Safety
/// Pass a live handle returned by create, exclusively on a serialized caller queue.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tdisk_scan_cancel(handle: *mut std::ffi::c_void) {
    if let Some(session) = unsafe { handle.cast::<Session>().as_ref() } {
        session.progress.cancelled.store(true, Ordering::Relaxed);
    }
}
/// # Safety
/// The handle must be live and exclusively owned; never use it after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tdisk_scan_free(handle: *mut std::ffi::c_void) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle.cast::<Session>()) });
    }
}
/// # Safety
/// Use a live exclusive handle and a NUL-terminated identifier (empty for root).
/// Free the returned string once with tdisk_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tdisk_scan_snapshot(
    handle: *mut std::ffi::c_void,
    id: *const c_char,
    previous: u64,
) -> *mut c_char {
    let (Some(session), Some(id)) = (unsafe { handle.cast::<Session>().as_mut() }, unsafe {
        input(id)
    }) else {
        return std::ptr::null_mut();
    };
    json_output(|| session.snapshot(id, previous))
}
/// # Safety
/// Use a live exclusive handle and valid NUL-terminated identifier. This prepares
/// a warning only; it does not move anything. Free the reply with tdisk_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tdisk_trash_prepare(
    handle: *mut std::ffi::c_void,
    id: *const c_char,
) -> *mut c_char {
    let (Some(session), Some(id)) = (unsafe { handle.cast::<Session>().as_mut() }, unsafe {
        input(id)
    }) else {
        return std::ptr::null_mut();
    };
    json_output(|| session.prepare_trash(id))
}
/// # Safety
/// Use a live exclusive handle, only after explicit confirmation of the prepared item.
/// Free the reply with tdisk_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tdisk_trash_execute(handle: *mut std::ffi::c_void) -> *mut c_char {
    let Some(session) = (unsafe { handle.cast::<Session>().as_mut() }) else {
        return std::ptr::null_mut();
    };
    json_output(|| session.trash())
}
/// # Safety
/// Use a live exclusive handle. Free the reply with tdisk_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tdisk_scan_issues(handle: *mut std::ffi::c_void) -> *mut c_char {
    let Some(session) = (unsafe { handle.cast::<Session>().as_mut() }) else {
        return std::ptr::null_mut();
    };
    json_output(|| session.issues())
}
/// # Safety
/// Pass only a string returned by this bridge, and release it exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tdisk_string_free(value: *mut c_char) {
    if !value.is_null() {
        drop(unsafe { CString::from_raw(value) });
    }
}
