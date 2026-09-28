//! Filesystem facade shared by every engine crate.
//!
//! Native builds re-export `std::fs` unchanged, so behaviour is identical.
//! `wasm32` builds keep an in-memory tree (`normalized path -> bytes`) that the
//! web entry point fills from a downloaded pack before the game starts.
//! `time` re-exports `web-time`, which is `std::time` on native.

pub mod time {
    pub use web_time::{Duration, Instant, SystemTime, SystemTimeError, UNIX_EPOCH};
}

use std::path::{Path, PathBuf};

/// Path queries routed through the facade (`p.vfs_is_file()` etc).
pub trait PathExt {
    fn vfs_exists(&self) -> bool;
    fn vfs_is_file(&self) -> bool;
    fn vfs_is_dir(&self) -> bool;
    fn vfs_metadata(&self) -> std::io::Result<Metadata>;
    fn vfs_canonicalize(&self) -> std::io::Result<PathBuf>;
    fn vfs_read_dir(&self) -> std::io::Result<ReadDir>;
}

impl PathExt for Path {
    fn vfs_exists(&self) -> bool { metadata(self).is_ok() }
    fn vfs_is_file(&self) -> bool { metadata(self).is_ok_and(|m| m.is_file()) }
    fn vfs_is_dir(&self) -> bool { metadata(self).is_ok_and(|m| m.is_dir()) }
    fn vfs_metadata(&self) -> std::io::Result<Metadata> { metadata(self) }
    fn vfs_canonicalize(&self) -> std::io::Result<PathBuf> { canonicalize(self) }
    fn vfs_read_dir(&self) -> std::io::Result<ReadDir> { read_dir(self) }
}

#[cfg(not(target_arch = "wasm32"))]
pub use std::fs::{
    DirEntry, File, FileType, Metadata, OpenOptions, ReadDir, canonicalize, copy, create_dir,
    create_dir_all, metadata, read, read_dir, read_to_string, remove_dir, remove_dir_all,
    remove_file, rename, symlink_metadata, write,
};

/// `std::env::temp_dir()`; `/tmp` in the in-memory tree on the web (std panics there).
pub fn temp_dir() -> PathBuf {
    #[cfg(target_arch = "wasm32")]
    return PathBuf::from("/tmp");
    #[cfg(not(target_arch = "wasm32"))]
    std::env::temp_dir()
}

/// `std::process::id()`; a fixed 1 on the web (std panics there).
pub fn process_id() -> u32 {
    #[cfg(target_arch = "wasm32")]
    return 1;
    #[cfg(not(target_arch = "wasm32"))]
    std::process::id()
}

/// `std::thread::sleep`; a no-op on the web, where the main thread may not block.
pub fn sleep(duration: time::Duration) {
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::sleep(duration);
    #[cfg(target_arch = "wasm32")]
    let _ = duration;
}

/// True when running on the in-memory filesystem (web build).
pub const IN_MEMORY: bool = cfg!(target_arch = "wasm32");

#[cfg(target_arch = "wasm32")]
pub use mem::*;

/// Parse an `SKPK1` pack and insert every entry below `root`.
/// Format: magic `SKPK1\0\0\0`, u32 count, then per entry u32 path_len,
/// utf8 path (relative, forward slashes), u64 data_len, bytes (all little endian).
pub fn install_pack(root: &Path, pack: &[u8]) -> Result<usize, String> {
    let mut at = 0usize;
    let mut take = |n: usize| -> Result<&[u8], String> {
        let end = at.checked_add(n).filter(|&e| e <= pack.len()).ok_or("Truncated pack")?;
        let slice = &pack[at..end];
        at = end;
        Ok(slice)
    };
    if take(8)? != b"SKPK1\0\0\0" { return Err("Not an SKPK1 pack".into()); }
    let count = u32::from_le_bytes(take(4)?.try_into().unwrap()) as usize;
    for _ in 0..count {
        let len = u32::from_le_bytes(take(4)?.try_into().unwrap()) as usize;
        let name = std::str::from_utf8(take(len)?).map_err(|_| "Pack path is not UTF-8")?.to_owned();
        let size = u64::from_le_bytes(take(8)?.try_into().unwrap()) as usize;
        let data = take(size)?;
        if name.starts_with('/') || name.split('/').any(|c| c == "..") {
            return Err(format!("Unsafe pack path {name}"));
        }
        insert(&root.join(&name), data.to_vec());
    }
    Ok(count)
}

/// Store bytes at `path` (creating parents); on native this writes the file.
pub fn insert(path: &Path, bytes: Vec<u8>) {
    #[cfg(target_arch = "wasm32")]
    mem::put(path, bytes.into());
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
        let _ = std::fs::write(path, bytes);
    }
}

#[cfg(target_arch = "wasm32")]
mod mem {
    use super::time::{SystemTime, UNIX_EPOCH};
    use std::{
        collections::{BTreeMap, BTreeSet},
        ffi::OsString,
        io::{self, Cursor, Read, Seek, SeekFrom},
        path::{Path, PathBuf},
        sync::{Arc, RwLock},
    };

    #[derive(Default)]
    struct Tree {
        files: BTreeMap<String, Arc<[u8]>>,
        dirs: BTreeSet<String>,
    }
    static TREE: RwLock<Tree> = RwLock::new(Tree { files: BTreeMap::new(), dirs: BTreeSet::new() });

    fn not_found(path: &str) -> io::Error { io::Error::new(io::ErrorKind::NotFound, format!("{path}: not found")) }

    /// `/a/b` form: forward slashes, no `.`/`..`, no trailing slash; relative paths hang off `/`.
    pub(crate) fn norm(path: &Path) -> String {
        let text = path.to_string_lossy().replace('\\', "/");
        let mut parts: Vec<&str> = Vec::new();
        for part in text.split('/') {
            match part {
                "" | "." => {}
                ".." => { parts.pop(); }
                p => parts.push(p),
            }
        }
        format!("/{}", parts.join("/"))
    }
    fn prefix(dir: &str) -> String { if dir == "/" { "/".into() } else { format!("{dir}/") } }
    impl Tree {
        fn is_dir(&self, key: &str) -> bool {
            if key == "/" || self.dirs.contains(key) { return true; }
            let p = prefix(key);
            self.files.range(p.clone()..).next().is_some_and(|(k, _)| k.starts_with(&p))
                || self.dirs.range(p.clone()..).next().is_some_and(|k| k.starts_with(&p))
        }
    }
    pub(crate) fn put(path: &Path, bytes: Arc<[u8]>) {
        TREE.write().unwrap().files.insert(norm(path), bytes);
    }

    /// Shared bytes without copying (web asset reader).
    pub fn read_shared(path: impl AsRef<Path>) -> io::Result<Arc<[u8]>> {
        let key = norm(path.as_ref());
        TREE.read().unwrap().files.get(&key).cloned().ok_or_else(|| not_found(&key))
    }
    pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> { read_shared(path).map(|b| b.to_vec()) }
    pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
        String::from_utf8(read(path)?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
    pub fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
        put(path.as_ref(), contents.as_ref().into());
        Ok(())
    }
    pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
        TREE.write().unwrap().dirs.insert(norm(path.as_ref()));
        Ok(())
    }
    pub fn create_dir(path: impl AsRef<Path>) -> io::Result<()> { create_dir_all(path) }
    pub fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
        let key = norm(path.as_ref());
        TREE.write().unwrap().files.remove(&key).map(|_| ()).ok_or_else(|| not_found(&key))
    }
    pub fn remove_dir(path: impl AsRef<Path>) -> io::Result<()> { remove_dir_all(path) }
    pub fn remove_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
        let key = norm(path.as_ref());
        let p = prefix(&key);
        let mut tree = TREE.write().unwrap();
        tree.files.retain(|k, _| !k.starts_with(&p));
        tree.dirs.retain(|k| k != &key && !k.starts_with(&p));
        Ok(())
    }
    pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
        let (from, to) = (norm(from.as_ref()), norm(to.as_ref()));
        let mut tree = TREE.write().unwrap();
        if let Some(bytes) = tree.files.remove(&from) {
            tree.files.insert(to, bytes);
            return Ok(());
        }
        let p = prefix(&from);
        let moved: Vec<_> = tree.files.keys().filter(|k| k.starts_with(&p)).cloned().collect();
        if moved.is_empty() && !tree.dirs.contains(&from) { return Err(not_found(&from)); }
        for k in moved {
            let bytes = tree.files.remove(&k).unwrap();
            tree.files.insert(format!("{to}/{}", &k[p.len()..]), bytes);
        }
        tree.dirs.remove(&from);
        tree.dirs.insert(to);
        Ok(())
    }
    pub fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<u64> {
        let bytes = read_shared(from)?;
        let n = bytes.len() as u64;
        put(to.as_ref(), bytes);
        Ok(n)
    }
    pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
        let key = norm(path.as_ref());
        let tree = TREE.read().unwrap();
        if tree.files.contains_key(&key) || tree.is_dir(&key) { Ok(PathBuf::from(key)) } else { Err(not_found(&key)) }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct FileType { dir: bool }
    impl FileType {
        pub fn is_dir(&self) -> bool { self.dir }
        pub fn is_file(&self) -> bool { !self.dir }
        pub fn is_symlink(&self) -> bool { false }
    }
    #[derive(Clone, Debug)]
    pub struct Metadata { len: u64, dir: bool }
    impl Metadata {
        pub fn len(&self) -> u64 { self.len }
        pub fn is_dir(&self) -> bool { self.dir }
        pub fn is_file(&self) -> bool { !self.dir }
        pub fn is_symlink(&self) -> bool { false }
        pub fn file_type(&self) -> FileType { FileType { dir: self.dir } }
        pub fn modified(&self) -> io::Result<SystemTime> { Ok(UNIX_EPOCH) }
        pub fn created(&self) -> io::Result<SystemTime> { Ok(UNIX_EPOCH) }
    }
    pub fn metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
        let key = norm(path.as_ref());
        let tree = TREE.read().unwrap();
        if let Some(b) = tree.files.get(&key) { return Ok(Metadata { len: b.len() as u64, dir: false }); }
        if tree.is_dir(&key) { return Ok(Metadata { len: 0, dir: true }); }
        Err(not_found(&key))
    }
    pub fn symlink_metadata(path: impl AsRef<Path>) -> io::Result<Metadata> { metadata(path) }

    pub struct DirEntry { path: PathBuf, meta: Metadata }
    impl DirEntry {
        pub fn path(&self) -> PathBuf { self.path.clone() }
        pub fn file_name(&self) -> OsString { self.path.file_name().unwrap_or_default().to_owned() }
        pub fn metadata(&self) -> io::Result<Metadata> { Ok(self.meta.clone()) }
        pub fn file_type(&self) -> io::Result<FileType> { Ok(self.meta.file_type()) }
    }
    pub struct ReadDir(std::vec::IntoIter<DirEntry>);
    impl Iterator for ReadDir {
        type Item = io::Result<DirEntry>;
        fn next(&mut self) -> Option<Self::Item> { self.0.next().map(Ok) }
    }
    /// Direct children, derived from stored keys. Entries keep the caller's spelling of `path`.
    pub fn read_dir(path: impl AsRef<Path>) -> io::Result<ReadDir> {
        let key = norm(path.as_ref());
        let tree = TREE.read().unwrap();
        if !tree.is_dir(&key) { return Err(not_found(&key)); }
        let p = prefix(&key);
        let mut children: BTreeMap<String, Metadata> = BTreeMap::new();
        for (k, b) in tree.files.range(p.clone()..).take_while(|(k, _)| k.starts_with(&p)) {
            let rest = &k[p.len()..];
            match rest.split_once('/') {
                Some((dir, _)) => { children.entry(dir.into()).or_insert(Metadata { len: 0, dir: true }); }
                None => { children.insert(rest.into(), Metadata { len: b.len() as u64, dir: false }); }
            }
        }
        for k in tree.dirs.range(p.clone()..).take_while(|k| k.starts_with(&p)) {
            let dir = k[p.len()..].split('/').next().unwrap_or_default();
            if !dir.is_empty() { children.entry(dir.into()).or_insert(Metadata { len: 0, dir: true }); }
        }
        let base = path.as_ref();
        Ok(ReadDir(children.into_iter().map(|(name, meta)| DirEntry { path: base.join(name), meta }).collect::<Vec<_>>().into_iter()))
    }

    /// Read-only file handle over stored bytes.
    pub struct File(Cursor<Arc<[u8]>>);
    impl File {
        pub fn open(path: impl AsRef<Path>) -> io::Result<Self> { Ok(Self(Cursor::new(read_shared(path)?))) }
        pub fn metadata(&self) -> io::Result<Metadata> { Ok(Metadata { len: self.0.get_ref().len() as u64, dir: false }) }
    }
    impl Read for File {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> { self.0.read(buf) }
    }
    impl Seek for File {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> { self.0.seek(pos) }
    }
    pub use std::fs::OpenOptions;
}
