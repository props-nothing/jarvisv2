use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use atomic_write_file::AtomicWriteFile;
use jarvis_core::{ClientCredential, CredentialError};

/// Filename of the profile-bound local client credential.
pub const CREDENTIAL_FILE_NAME: &str = "client.credential";

/// Explains why a credential could not be loaded, created, or secured.
#[derive(Debug, thiserror::Error)]
pub enum CredentialStoreError {
    /// The credential file could not be read.
    #[error("failed to read the local client credential file")]
    Read {
        /// The underlying operating-system error.
        #[source]
        source: io::Error,
    },
    /// The credential file could not be written atomically.
    #[error("failed to write the local client credential file")]
    Write {
        /// The underlying operating-system error.
        #[source]
        source: io::Error,
    },
    /// The stored credential text was not a valid credential.
    #[error("the stored local client credential is invalid")]
    Invalid(#[from] CredentialError),
    /// The credential file could not be hardened to user-only access.
    #[error("the local client credential file is not private to the current user")]
    Insecure {
        /// The underlying operating-system error.
        #[source]
        source: io::Error,
    },
}

/// Stores the profile-bound credential in a private configuration file.
#[derive(Clone, Debug)]
pub struct CredentialStore {
    path: PathBuf,
}

impl CredentialStore {
    /// Creates a store for an explicit credential file path.
    #[must_use]
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Returns the credential file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the stored credential.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialStoreError`] when the file cannot be read or the
    /// stored text is not a valid credential.
    pub fn load(&self) -> Result<ClientCredential, CredentialStoreError> {
        let text = fs::read_to_string(&self.path)
            .map_err(|source| CredentialStoreError::Read { source })?;
        ClientCredential::parse(text.trim_end()).map_err(CredentialStoreError::Invalid)
    }

    /// Loads the stored credential, generating and persisting one on first use.
    ///
    /// The write is atomic and the file is restricted to the current user before
    /// the value is returned, so a partially written secret is never observed.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialStoreError`] when an existing credential is invalid
    /// or a new one cannot be generated, written, or secured.
    pub fn load_or_create(&self, parent: &Path) -> Result<ClientCredential, CredentialStoreError> {
        match self.load() {
            Ok(credential) => Ok(credential),
            Err(CredentialStoreError::Read { source })
                if source.kind() == io::ErrorKind::NotFound =>
            {
                let credential =
                    ClientCredential::generate().map_err(CredentialStoreError::Invalid)?;
                self.store(parent, &credential)?;
                Ok(credential)
            }
            Err(other) => Err(other),
        }
    }

    /// Reports whether a stored credential exists and is usable, without reading
    /// the value into a caller-visible form.
    ///
    /// Doctor uses this to prove the credential is present and well-formed while
    /// never returning the secret.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialStoreError::Read`] when the file exists but cannot be
    /// read, or [`CredentialStoreError::Invalid`] when it is not a credential.
    pub fn inspect_presence(&self) -> Result<bool, CredentialStoreError> {
        match self.load() {
            Ok(_) => Ok(true),
            Err(CredentialStoreError::Read { source })
                if source.kind() == io::ErrorKind::NotFound =>
            {
                Ok(false)
            }
            Err(other) => Err(other),
        }
    }

    /// Atomically writes a credential with user-only access.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialStoreError`] when the directory cannot be created,
    /// the write fails, or the file cannot be hardened.
    pub fn store(
        &self,
        parent: &Path,
        credential: &ClientCredential,
    ) -> Result<(), CredentialStoreError> {
        fs::create_dir_all(parent).map_err(|source| CredentialStoreError::Write { source })?;

        let mut file = AtomicWriteFile::open(&self.path)
            .map_err(|source| CredentialStoreError::Write { source })?;
        file.write_all(credential.expose().as_bytes())
            .map_err(|source| CredentialStoreError::Write { source })?;
        file.commit()
            .map_err(|source| CredentialStoreError::Write { source })?;

        secure(&self.path).map_err(|source| CredentialStoreError::Insecure { source })
    }
}

#[cfg(unix)]
fn secure(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(windows)]
fn secure(path: &Path) -> io::Result<()> {
    // The configuration directory is already restricted to the current user by
    // `AppPaths::prepare`; a readable check here keeps the promise explicit.
    let metadata = fs::metadata(path)?;
    if metadata.is_file() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "credential path is not a regular file",
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEST_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = TEST_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "jarvis-storage-credential-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create temp dir: {error}"));
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_first_run_generates_and_persists_a_credential() {
        let directory = TestDirectory::new();
        let store = CredentialStore::at(directory.0.join(CREDENTIAL_FILE_NAME));

        let created = store
            .load_or_create(&directory.0)
            .unwrap_or_else(|error| panic!("create: {error}"));
        let loaded = store
            .load_or_create(&directory.0)
            .unwrap_or_else(|error| panic!("reload: {error}"));

        assert_eq!(created, loaded);
        assert!(store.path().exists());
    }

    #[test]
    fn a_tampered_credential_file_fails_closed() {
        let directory = TestDirectory::new();
        let store = CredentialStore::at(directory.0.join(CREDENTIAL_FILE_NAME));
        fs::write(store.path(), "not-a-credential")
            .unwrap_or_else(|error| panic!("write fixture: {error}"));

        let result = store.load_or_create(&directory.0);
        assert!(matches!(
            result,
            Err(CredentialStoreError::Invalid(
                CredentialError::InvalidLength
            ))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_stored_credential_is_not_readable_by_other_users() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TestDirectory::new();
        let store = CredentialStore::at(directory.0.join(CREDENTIAL_FILE_NAME));
        let credential =
            ClientCredential::generate().unwrap_or_else(|error| panic!("generate: {error}"));
        store
            .store(&directory.0, &credential)
            .unwrap_or_else(|error| panic!("store: {error}"));

        let mode = fs::metadata(store.path())
            .unwrap_or_else(|error| panic!("metadata: {error}"))
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o077,
            0,
            "credential must not be group/world readable"
        );
    }
}
