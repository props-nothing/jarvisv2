use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Write},
    path::Path,
};

use jarvis_storage::{PathError, PathKind, secure_private_file};
use thiserror::Error;

use crate::build_info::BuildInfo;

const MAX_LOCK_METADATA_BYTES: usize = 256;

#[derive(Debug, Error)]
pub(crate) enum SingletonError {
    #[error("the daemon lock path is not absolute")]
    RelativePath,
    #[error("the daemon lock path has no parent directory")]
    MissingParent,
    #[error("the daemon lock path is a symbolic link")]
    SymbolicLink,
    #[error("the daemon lock path is not a regular file")]
    NotFile,
    #[error("another jarvisd process already owns this profile")]
    AlreadyRunning,
    #[error("failed to {operation} the daemon lock file")]
    Io {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("failed to secure the daemon lock file")]
    Secure(#[source] PathError),
}

#[derive(Debug)]
pub(crate) struct SingletonGuard {
    file: File,
}

impl SingletonGuard {
    pub(crate) fn acquire(path: &Path, build: BuildInfo) -> Result<Self, SingletonError> {
        inspect_endpoint(path)?;
        let file = open_lock_file(path)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(SingletonError::AlreadyRunning),
            Err(TryLockError::Error(source)) => {
                return Err(SingletonError::Io {
                    operation: "lock",
                    source,
                });
            }
        }
        secure_private_file(PathKind::Runtime, path).map_err(SingletonError::Secure)?;
        write_metadata(&file, build)?;
        Ok(Self { file })
    }

    pub(crate) fn release(self) -> Result<(), SingletonError> {
        self.file.unlock().map_err(|source| SingletonError::Io {
            operation: "unlock",
            source,
        })
    }
}

fn inspect_endpoint(path: &Path) -> Result<(), SingletonError> {
    if !path.is_absolute() {
        return Err(SingletonError::RelativePath);
    }
    if path.parent().is_none() {
        return Err(SingletonError::MissingParent);
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(SingletonError::SymbolicLink),
        Ok(metadata) if !metadata.is_file() => Err(SingletonError::NotFile),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(SingletonError::Io {
            operation: "inspect",
            source,
        }),
    }
}

fn open_lock_file(path: &Path) -> Result<File, SingletonError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(|source| SingletonError::Io {
        operation: "open",
        source,
    })
}

fn write_metadata(file: &File, build: BuildInfo) -> Result<(), SingletonError> {
    let metadata = format!(
        "pid={}\nversion={}\ntarget={}-{}\n",
        std::process::id(),
        build.version(),
        build.target_arch(),
        build.target_os()
    );
    if metadata.len() > MAX_LOCK_METADATA_BYTES {
        return Err(SingletonError::Io {
            operation: "bound metadata",
            source: io::Error::new(io::ErrorKind::InvalidData, "lock metadata is too large"),
        });
    }
    file.set_len(0).map_err(|source| SingletonError::Io {
        operation: "truncate",
        source,
    })?;
    let mut writer = file;
    writer
        .write_all(metadata.as_bytes())
        .map_err(|source| SingletonError::Io {
            operation: "write",
            source,
        })?;
    file.sync_data().map_err(|source| SingletonError::Io {
        operation: "synchronize",
        source,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("jarvisd-singleton-{}", jarvis_core::scratch_tag()));
            if let Err(error) = fs::create_dir_all(&path) {
                panic!("create singleton test directory: {error}");
            }
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            jarvis_core::remove_scratch_dir(&self.0);
        }
    }

    #[test]
    fn second_guard_is_denied_until_first_handle_closes() {
        let directory = TestDirectory::new();
        let path = directory.0.join("jarvisd.lock");
        let first = SingletonGuard::acquire(&path, BuildInfo::current())
            .unwrap_or_else(|error| panic!("acquire first guard: {error}"));

        let second = SingletonGuard::acquire(&path, BuildInfo::current());
        assert!(matches!(second, Err(SingletonError::AlreadyRunning)));
        if let Err(error) = first.release() {
            panic!("release first guard: {error}");
        }

        assert!(path.is_file());
        let metadata =
            fs::read_to_string(&path).unwrap_or_else(|error| panic!("read lock metadata: {error}"));
        assert!(metadata.contains(&format!("pid={}", std::process::id())));
        assert!(metadata.contains("version=0.1.0"));
        assert!(metadata.len() <= MAX_LOCK_METADATA_BYTES);

        let third = SingletonGuard::acquire(&path, BuildInfo::current())
            .unwrap_or_else(|error| panic!("reacquire released guard: {error}"));
        if let Err(error) = third.release() {
            panic!("release third guard: {error}");
        }
    }

    #[test]
    fn directory_at_lock_endpoint_is_rejected() {
        let directory = TestDirectory::new();
        let path = directory.0.join("jarvisd.lock");
        if let Err(error) = fs::create_dir(&path) {
            panic!("create invalid lock directory: {error}");
        }

        let result = SingletonGuard::acquire(&path, BuildInfo::current());
        assert!(matches!(result, Err(SingletonError::NotFile)));
    }
}
