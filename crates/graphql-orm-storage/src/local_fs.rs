//! Root-relative filesystem access for local storage that never follows a
//! link below the root.
//!
//! Every operation starts from a handle on the configured root and reaches
//! its target one component at a time, relative to a directory handle it
//! already holds. No operation below the root uses a full path:
//!
//! - On Unix each directory is opened with `openat(O_DIRECTORY |
//!   O_NOFOLLOW)` relative to its parent's descriptor, and leaf operations
//!   are `*at` calls on the parent descriptor (`openat(O_NOFOLLOW)`,
//!   `renameat`, `linkat`, `unlinkat`, `mkdirat`, `fstatat` without
//!   following). On Linux a directory chain is resolved in one
//!   `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)` call where the kernel
//!   supports it. A directory replaced by a link after it was opened is
//!   never entered: the operation keeps using the descriptor it holds.
//! - On Windows, which has no portable handle-relative open in the standard
//!   library, each directory from the root down is opened as itself
//!   (`FILE_FLAG_OPEN_REPARSE_POINT`), refused when it is a reparse point,
//!   and held without `FILE_SHARE_DELETE` for the whole operation, so no
//!   directory on the chain can be renamed, removed or replaced while the
//!   operation runs. Leaf files are opened as themselves too, and the held
//!   parent is checked again after a leaf is created or opened, so an empty
//!   directory turned into a junction in between is refused.
//!
//! The configured root itself is opened by its path and may be a link: the
//! host chose it.
//!
//! A refused entry is reported as an [`io::Error`] carrying [`Refused`],
//! which callers map to `StorageError::InvalidStorageKey`; see
//! [`is_refusal`].

use std::{
    ffi::OsString,
    fmt,
    fs::File,
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// Why an entry below the root was refused.
#[derive(Debug)]
pub(crate) struct Refused(&'static str);

impl fmt::Display for Refused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for Refused {}

/// A refusal of a link, or of a file with more than one hard link.
pub(crate) fn refused(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, Refused(reason))
}

/// Whether `error` is a refusal from this module.
pub(crate) fn is_refusal(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|inner| inner.is::<Refused>())
}

const LINK: &str = "local storage refuses filesystem links below its root";
const HARD_LINK: &str = "local storage refuses files with more than one hard link";

/// The kind of a directory entry, never following a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    File,
    Dir,
    /// A symbolic link, or on Windows any name-surrogate reparse point
    /// (junction, mount point) or reparse-point directory.
    Link,
    /// A FIFO, socket, device or anything else.
    Other,
}

/// What a directory entry is, inspected without following it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Info {
    pub(crate) kind: Kind,
    pub(crate) len: u64,
    pub(crate) modified: Option<SystemTime>,
    /// Hard links to a file.
    pub(crate) links: u64,
    /// The file's identity on its volume: device and inode on Unix, volume
    /// serial and file index on Windows.
    pub(crate) identity: (u64, u64),
}

/// One entry of a directory.
pub(crate) struct Entry {
    pub(crate) name: OsString,
    pub(crate) kind: Kind,
}

/// A directory at or below the root, held open.
pub(crate) struct Dir {
    path: PathBuf,
    handle: sys::Handle,
}

impl fmt::Debug for Dir {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Dir")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Dir {
    /// Opens the configured root, following it if it is a link. Returns
    /// `None` when it does not exist.
    pub(crate) fn open_root(root: &Path) -> io::Result<Option<Self>> {
        match sys::open_root(root) {
            Ok(handle) => Ok(Some(Self {
                path: root.to_path_buf(),
                handle,
            })),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// The directory's path, for diagnostics and tracing only.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// The path of the entry `name` in this directory, for diagnostics only.
    pub(crate) fn child_path(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// Opens the child directory `name`, or `None` when it does not exist or
    /// is not a directory. A link is refused.
    pub(crate) fn child_dir(&self, name: &str) -> io::Result<Option<Self>> {
        Ok(sys::child_dir(self, name)?.map(|handle| Self {
            path: self.path.join(name),
            handle,
        }))
    }

    /// Opens the directory `components` below this one, or `None` when one
    /// of them does not exist or is not a directory. A link anywhere on the
    /// way is refused.
    pub(crate) fn descend(&self, components: &[&str]) -> io::Result<Option<Self>> {
        if components.is_empty() {
            return Ok(Some(self.try_clone()?));
        }
        if let Some(found) = sys::descend_at_once(self, components)? {
            return Ok(found.map(|handle| Self {
                path: components
                    .iter()
                    .fold(self.path.clone(), |path, name| path.join(name)),
                handle,
            }));
        }
        let mut current = match self.child_dir(components[0])? {
            Some(dir) => dir,
            None => return Ok(None),
        };
        for name in &components[1..] {
            current = match current.child_dir(name)? {
                Some(dir) => dir,
                None => return Ok(None),
            };
        }
        Ok(Some(current))
    }

    /// Another handle on this directory.
    pub(crate) fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            path: self.path.clone(),
            handle: sys::try_clone(&self.handle)?,
        })
    }

    /// Creates the child directory `name`. Returns `false` when an entry of
    /// that name already exists.
    pub(crate) fn create_dir(&self, name: &str) -> io::Result<bool> {
        sys::create_dir(self, name)
    }

    /// Opens the regular file `name` for reading, or `None` when it does not
    /// exist. A link, or a file with more than one hard link, is refused; a
    /// directory, FIFO, socket or device is an error.
    pub(crate) fn open_read(&self, name: &str) -> io::Result<Option<File>> {
        sys::open_read(self, name)
    }

    /// What the entry `name` is, or `None` when it does not exist.
    pub(crate) fn info(&self, name: &str) -> io::Result<Option<Info>> {
        sys::info(self, name)
    }

    /// Creates the new file `name` for writing; an existing entry, link or
    /// not, is an error.
    pub(crate) fn create_new(&self, name: &str) -> io::Result<File> {
        sys::create_new(self, name)
    }

    /// Opens or creates the file `name` for reading and writing, without
    /// truncating it. A link is refused.
    pub(crate) fn open_or_create(&self, name: &str) -> io::Result<File> {
        sys::open_or_create(self, name)
    }

    /// Renames `from` to `to` in this directory, replacing `to`.
    pub(crate) fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        sys::rename(self, from, to)
    }

    /// Renames `from` to `to` in this directory unless `to` exists, which
    /// fails with [`io::ErrorKind::AlreadyExists`].
    ///
    /// Linux and Apple platforms rename without replacing in one call
    /// (`RENAME_NOREPLACE`, `RENAME_EXCL`), so `to` never has a second hard
    /// link. Elsewhere, or on a file system that refuses that flag, `from` is
    /// linked at `to` and then unlinked, and for that moment `to` has two
    /// hard links.
    pub(crate) fn rename_noreplace(&self, from: &str, to: &str) -> io::Result<()> {
        sys::rename_noreplace(self, from, to)
    }

    /// Removes the file or link `name`.
    pub(crate) fn remove_file(&self, name: &str) -> io::Result<()> {
        sys::remove_file(self, name)
    }

    /// Removes the empty directory `name`.
    pub(crate) fn remove_dir(&self, name: &str) -> io::Result<()> {
        sys::remove_dir(self, name)
    }

    /// Removes the directory `name` and everything below it, without
    /// following links: a link inside is removed, not its target.
    pub(crate) fn remove_tree(&self, name: &str) -> io::Result<()> {
        let Some(child) = self.child_dir(name)? else {
            return match self.info(name)? {
                None => Ok(()),
                Some(_) => self.remove_file(name),
            };
        };
        for entry in child.entries()? {
            let Some(entry_name) = entry.name.to_str() else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "entry name is not valid Unicode",
                ));
            };
            match entry.kind {
                Kind::Dir => child.remove_tree(entry_name)?,
                _ => match child.remove_file(entry_name) {
                    Ok(()) => {}
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                    Err(err) => return Err(err),
                },
            }
        }
        drop(child);
        match self.remove_dir(name) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }

    /// Makes entry changes in this directory durable.
    pub(crate) fn sync(&self) -> io::Result<()> {
        sys::sync(&self.handle)
    }

    /// The entries of this directory, without `.` and `..`, in no order.
    pub(crate) fn entries(&self) -> io::Result<Vec<Entry>> {
        let mut entries = Vec::new();
        self.for_each_entry(|entry| {
            entries.push(entry);
            Ok(())
        })?;
        Ok(entries)
    }

    /// Calls `visit` with each entry of this directory, without `.` and
    /// `..`, in no order, holding one entry at a time.
    pub(crate) fn for_each_entry(
        &self,
        visit: impl FnMut(Entry) -> io::Result<()>,
    ) -> io::Result<()> {
        sys::for_each_entry(self, visit)
    }
}

/// Whether `info` is a file a read may open: a regular file with one hard
/// link.
fn check_readable(info: &Info) -> io::Result<()> {
    match info.kind {
        Kind::Link => Err(refused(LINK)),
        Kind::File if info.links > 1 => Err(refused(HARD_LINK)),
        Kind::File => Ok(()),
        Kind::Dir => Err(io::Error::from(io::ErrorKind::IsADirectory)),
        Kind::Other => Err(io::Error::from(io::ErrorKind::InvalidInput)),
    }
}

#[cfg(unix)]
mod sys {
    use std::{
        ffi::OsString,
        fs::File,
        io,
        os::{
            fd::{AsFd, OwnedFd},
            unix::ffi::OsStringExt,
        },
        path::Path,
        time::{Duration, SystemTime},
    };

    use rustix::{
        fs::{self as rfs, AtFlags, FileType, Mode, OFlags},
        io::Errno,
    };

    use super::{Dir, Entry, Info, Kind, LINK, check_readable, refused};

    pub(super) struct Handle(OwnedFd);

    const DIRECTORY: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);

    fn fd(dir: &Dir) -> std::os::fd::BorrowedFd<'_> {
        dir.handle.0.as_fd()
    }

    pub(super) fn open_root(root: &Path) -> io::Result<Handle> {
        rfs::open(
            root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map(Handle)
        .map_err(io::Error::from)
    }

    pub(super) fn try_clone(handle: &Handle) -> io::Result<Handle> {
        handle.0.try_clone().map(Handle)
    }

    /// Whether `name` in `dir` is a symbolic link.
    fn is_symlink(dir: &Dir, name: &str) -> bool {
        rfs::statat(fd(dir), name, AtFlags::SYMLINK_NOFOLLOW)
            .is_ok_and(|stat| FileType::from_raw_mode(stat.st_mode) == FileType::Symlink)
    }

    pub(super) fn child_dir(dir: &Dir, name: &str) -> io::Result<Option<Handle>> {
        match rfs::openat(fd(dir), name, DIRECTORY, Mode::empty()) {
            Ok(handle) => Ok(Some(Handle(handle))),
            Err(Errno::NOENT) => Ok(None),
            Err(Errno::LOOP) => Err(refused(LINK)),
            Err(Errno::NOTDIR) if is_symlink(dir, name) => Err(refused(LINK)),
            Err(Errno::NOTDIR) => Ok(None),
            Err(errno) => Err(errno.into()),
        }
    }

    /// Resolves `components` below `dir` in one `openat2` call that refuses
    /// every link and any escape from `dir`. Returns `None` when the kernel
    /// cannot do that, so the caller walks the components one at a time.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub(super) fn descend_at_once(
        dir: &Dir,
        components: &[&str],
    ) -> io::Result<Option<Option<Handle>>> {
        use std::sync::atomic::{AtomicBool, Ordering};

        use rustix::fs::ResolveFlags;

        static UNAVAILABLE: AtomicBool = AtomicBool::new(false);
        if UNAVAILABLE.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let joined = components.join("/");
        match rfs::openat2(
            fd(dir),
            joined.as_str(),
            DIRECTORY,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        ) {
            Ok(handle) => Ok(Some(Some(Handle(handle)))),
            Err(Errno::NOENT) => Ok(Some(None)),
            // A link as the last component is reported as not a directory
            // under `O_NOFOLLOW`: walk, which tells a link from a file.
            Err(Errno::NOTDIR) => Ok(None),
            Err(Errno::LOOP | Errno::XDEV) => Err(refused(LINK)),
            // No `openat2`, or a sandbox that refuses it.
            Err(Errno::NOSYS | Errno::PERM) => {
                UNAVAILABLE.store(true, Ordering::Relaxed);
                Ok(None)
            }
            // A rename race the kernel detected, or flags it does not know:
            // walk instead.
            Err(Errno::AGAIN | Errno::INVAL | Errno::TOOBIG) => Ok(None),
            Err(errno) => Err(errno.into()),
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    pub(super) fn descend_at_once(
        _dir: &Dir,
        _components: &[&str],
    ) -> io::Result<Option<Option<Handle>>> {
        Ok(None)
    }

    pub(super) fn create_dir(dir: &Dir, name: &str) -> io::Result<bool> {
        match rfs::mkdirat(fd(dir), name, Mode::from_raw_mode(0o777)) {
            Ok(()) => Ok(true),
            Err(Errno::EXIST) => Ok(false),
            Err(errno) => Err(errno.into()),
        }
    }

    pub(super) fn open_read(dir: &Dir, name: &str) -> io::Result<Option<File>> {
        // `O_NONBLOCK` keeps a FIFO from blocking the open; it is refused
        // below. It does not affect reads of a regular file.
        let flags =
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC;
        let handle = match rfs::openat(fd(dir), name, flags, Mode::empty()) {
            Ok(handle) => handle,
            Err(Errno::NOENT) => return Ok(None),
            Err(Errno::LOOP) => return Err(refused(LINK)),
            Err(Errno::NXIO) => return Err(io::Error::from(io::ErrorKind::InvalidInput)),
            Err(errno) => return Err(errno.into()),
        };
        check_readable(&from_stat(&rfs::fstat(&handle)?))?;
        Ok(Some(File::from(handle)))
    }

    pub(super) fn info(dir: &Dir, name: &str) -> io::Result<Option<Info>> {
        match rfs::statat(fd(dir), name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => Ok(Some(from_stat(&stat))),
            Err(Errno::NOENT | Errno::NOTDIR) => Ok(None),
            Err(errno) => Err(errno.into()),
        }
    }

    pub(super) fn create_new(dir: &Dir, name: &str) -> io::Result<File> {
        rfs::openat(
            fd(dir),
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o666),
        )
        .map(File::from)
        .map_err(io::Error::from)
    }

    pub(super) fn open_or_create(dir: &Dir, name: &str) -> io::Result<File> {
        match rfs::openat(
            fd(dir),
            name,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o666),
        ) {
            Ok(handle) => Ok(File::from(handle)),
            Err(Errno::LOOP) => Err(refused(LINK)),
            Err(errno) => Err(errno.into()),
        }
    }

    pub(super) fn rename(dir: &Dir, from: &str, to: &str) -> io::Result<()> {
        rfs::renameat(fd(dir), from, fd(dir), to).map_err(io::Error::from)
    }

    pub(super) fn rename_noreplace(dir: &Dir, from: &str, to: &str) -> io::Result<()> {
        #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
        match rfs::renameat_with(fd(dir), from, fd(dir), to, rfs::RenameFlags::NOREPLACE) {
            Ok(()) => return Ok(()),
            Err(Errno::EXIST) => return Err(io::Error::from(io::ErrorKind::AlreadyExists)),
            // The file system cannot rename without replacing: link instead.
            Err(Errno::INVAL | Errno::NOSYS | Errno::NOTSUP) => {}
            Err(errno) => return Err(errno.into()),
        }
        match rfs::linkat(fd(dir), from, fd(dir), to, AtFlags::empty()) {
            Ok(()) => {}
            Err(Errno::EXIST) => return Err(io::Error::from(io::ErrorKind::AlreadyExists)),
            Err(errno) => return Err(errno.into()),
        }
        let _ = rfs::unlinkat(fd(dir), from, AtFlags::empty());
        Ok(())
    }

    pub(super) fn remove_file(dir: &Dir, name: &str) -> io::Result<()> {
        rfs::unlinkat(fd(dir), name, AtFlags::empty()).map_err(io::Error::from)
    }

    pub(super) fn remove_dir(dir: &Dir, name: &str) -> io::Result<()> {
        rfs::unlinkat(fd(dir), name, AtFlags::REMOVEDIR).map_err(io::Error::from)
    }

    pub(super) fn sync(handle: &Handle) -> io::Result<()> {
        rfs::fsync(&handle.0).map_err(io::Error::from)
    }

    pub(super) fn for_each_entry(
        dir: &Dir,
        mut visit: impl FnMut(Entry) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut entries = rfs::Dir::read_from(fd(dir))?;
        while let Some(entry) = entries.read() {
            let entry = entry?;
            let name = entry.file_name().to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            let name = OsString::from_vec(name.to_vec());
            let kind = match entry.file_type() {
                FileType::Unknown => match name
                    .to_str()
                    .map(|name| rfs::statat(fd(dir), name, AtFlags::SYMLINK_NOFOLLOW))
                {
                    Some(Ok(stat)) => kind(FileType::from_raw_mode(stat.st_mode)),
                    Some(Err(Errno::NOENT)) => continue,
                    Some(Err(errno)) => return Err(errno.into()),
                    None => Kind::Other,
                },
                file_type => kind(file_type),
            };
            visit(Entry { name, kind })?;
        }
        Ok(())
    }

    fn kind(file_type: FileType) -> Kind {
        match file_type {
            FileType::RegularFile => Kind::File,
            FileType::Directory => Kind::Dir,
            FileType::Symlink => Kind::Link,
            _ => Kind::Other,
        }
    }

    // The field types of `stat` differ between targets.
    #[allow(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        clippy::unnecessary_cast,
        clippy::useless_conversion
    )]
    fn from_stat(stat: &rfs::Stat) -> Info {
        let seconds = stat.st_mtime as i64;
        let nanos = stat.st_mtime_nsec as u32;
        let modified = if seconds >= 0 {
            SystemTime::UNIX_EPOCH.checked_add(Duration::new(seconds as u64, nanos))
        } else {
            SystemTime::UNIX_EPOCH
                .checked_sub(Duration::from_secs(seconds.unsigned_abs()))
                .and_then(|time| time.checked_add(Duration::from_nanos(u64::from(nanos))))
        };
        Info {
            kind: kind(FileType::from_raw_mode(stat.st_mode)),
            len: stat.st_size as u64,
            modified,
            links: stat.st_nlink as u64,
            identity: (stat.st_dev as u64, stat.st_ino as u64),
        }
    }
}

#[cfg(windows)]
mod sys {
    use std::{
        fs::{File, OpenOptions},
        io,
        os::windows::fs::{MetadataExt, OpenOptionsExt},
        path::Path,
        sync::Arc,
    };

    use super::{Dir, Entry, Info, Kind, LINK, check_readable, refused};

    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2;
    const FILE_SHARE_DELETE: u32 = 0x4;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

    /// Every directory from the root to this one, held open without
    /// `FILE_SHARE_DELETE`, so none of them can be renamed or removed while
    /// the handle lives.
    pub(super) struct Handle(Arc<Vec<File>>);

    impl Handle {
        fn last(&self) -> &File {
            self.0.last().expect("a held directory")
        }
    }

    fn open_directory(path: &Path, follow: bool) -> io::Result<File> {
        let mut flags = FILE_FLAG_BACKUP_SEMANTICS;
        if !follow {
            flags |= FILE_FLAG_OPEN_REPARSE_POINT;
        }
        OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(flags)
            .open(path)
    }

    fn attributes(file: &File) -> io::Result<u32> {
        Ok(file.metadata()?.file_attributes())
    }

    /// Refuses when the held directory became a reparse point, as an empty
    /// directory can, after it was opened.
    fn check_intact(dir: &Dir) -> io::Result<()> {
        if attributes(dir.handle.last())? & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(refused(LINK));
        }
        Ok(())
    }

    pub(super) fn open_root(root: &Path) -> io::Result<Handle> {
        Ok(Handle(Arc::new(vec![open_directory(root, true)?])))
    }

    pub(super) fn try_clone(handle: &Handle) -> io::Result<Handle> {
        Ok(Handle(Arc::clone(&handle.0)))
    }

    pub(super) fn child_dir(dir: &Dir, name: &str) -> io::Result<Option<Handle>> {
        let child = match open_directory(&dir.path.join(name), false) {
            Ok(child) => child,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        let metadata = child.metadata()?;
        if metadata.file_type().is_symlink()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return Err(refused(LINK));
        }
        if metadata.file_attributes() & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Ok(None);
        }
        check_intact(dir)?;
        let mut chain = Vec::with_capacity(dir.handle.0.len() + 1);
        for held in dir.handle.0.iter() {
            chain.push(held.try_clone()?);
        }
        chain.push(child);
        Ok(Some(Handle(Arc::new(chain))))
    }

    pub(super) fn descend_at_once(
        _dir: &Dir,
        _components: &[&str],
    ) -> io::Result<Option<Option<Handle>>> {
        Ok(None)
    }

    pub(super) fn create_dir(dir: &Dir, name: &str) -> io::Result<bool> {
        let created = match std::fs::create_dir(dir.path.join(name)) {
            Ok(()) => true,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => false,
            Err(err) => return Err(err),
        };
        check_intact(dir)?;
        Ok(created)
    }

    fn identity(file: &File) -> io::Result<(u64, u64, u64)> {
        let information = winapi_util::file::information(file)?;
        Ok((
            information.volume_serial_number(),
            information.file_index(),
            information.number_of_links(),
        ))
    }

    fn info_of(file: &File) -> io::Result<Info> {
        let metadata = file.metadata()?;
        let attributes = metadata.file_attributes();
        let kind = if metadata.file_type().is_symlink()
            || (attributes & FILE_ATTRIBUTE_DIRECTORY != 0
                && attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0)
        {
            Kind::Link
        } else if metadata.is_dir() {
            Kind::Dir
        } else if metadata.is_file() {
            Kind::File
        } else {
            Kind::Other
        };
        let (volume, index, links) = identity(file)?;
        Ok(Info {
            kind,
            len: metadata.len(),
            modified: metadata.modified().ok(),
            links,
            identity: (volume, index),
        })
    }

    pub(super) fn open_read(dir: &Dir, name: &str) -> io::Result<Option<File>> {
        let path = dir.path.join(name);
        let file = match OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)
        {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        let info = info_of(&file)?;
        check_readable(&info)?;
        check_intact(dir)?;
        if attributes(&file)? & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
            return Ok(Some(file));
        }
        // Another kind of reparse point, such as a deduplicated or cloud
        // file, is read through its file system filter, and must be the same
        // file.
        let reopened = match File::open(&path) {
            Ok(reopened) => reopened,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        let (volume, index, _) = identity(&reopened)?;
        if (volume, index) != info.identity {
            return Err(refused(LINK));
        }
        Ok(Some(reopened))
    }

    pub(super) fn info(dir: &Dir, name: &str) -> io::Result<Option<Info>> {
        let file = match OpenOptions::new()
            .access_mode(0)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(dir.path.join(name))
        {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        let info = info_of(&file)?;
        check_intact(dir)?;
        Ok(Some(info))
    }

    pub(super) fn create_new(dir: &Dir, name: &str) -> io::Result<File> {
        let path = dir.path.join(name);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)?;
        if let Err(err) = check_intact(dir) {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(err);
        }
        Ok(file)
    }

    pub(super) fn open_or_create(dir: &Dir, name: &str) -> io::Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(dir.path.join(name))?;
        if info_of(&file)?.kind == Kind::Link {
            return Err(refused(LINK));
        }
        check_intact(dir)?;
        Ok(file)
    }

    pub(super) fn rename(dir: &Dir, from: &str, to: &str) -> io::Result<()> {
        std::fs::rename(dir.path.join(from), dir.path.join(to))
    }

    pub(super) fn rename_noreplace(dir: &Dir, from: &str, to: &str) -> io::Result<()> {
        std::fs::hard_link(dir.path.join(from), dir.path.join(to))?;
        let _ = std::fs::remove_file(dir.path.join(from));
        Ok(())
    }

    pub(super) fn remove_file(dir: &Dir, name: &str) -> io::Result<()> {
        std::fs::remove_file(dir.path.join(name))
    }

    pub(super) fn remove_dir(dir: &Dir, name: &str) -> io::Result<()> {
        std::fs::remove_dir(dir.path.join(name))
    }

    /// Windows exposes no directory sync through the standard library; the
    /// file system persists directory entries itself.
    pub(super) fn sync(_handle: &Handle) -> io::Result<()> {
        Ok(())
    }

    pub(super) fn for_each_entry(
        dir: &Dir,
        mut visit: impl FnMut(Entry) -> io::Result<()>,
    ) -> io::Result<()> {
        for entry in std::fs::read_dir(&dir.path)? {
            let entry = entry?;
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err),
            };
            let attributes = metadata.file_attributes();
            let kind = if metadata.file_type().is_symlink()
                || attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
                    && attributes & FILE_ATTRIBUTE_DIRECTORY != 0
            {
                Kind::Link
            } else if metadata.is_dir() {
                Kind::Dir
            } else if metadata.is_file() {
                Kind::File
            } else {
                Kind::Other
            };
            visit(Entry {
                name: entry.file_name(),
                kind,
            })?;
        }
        check_intact(dir)
    }
}

#[cfg(not(any(unix, windows)))]
compile_error!("local storage supports Unix and Windows targets only");
