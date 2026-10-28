//! User-local authentication state. Never include credential contents in diagnostics.
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

const MAX_BYTES: usize = 64 * 1024;

#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct CredentialRecord {
    pub client_id: String,
    pub subject: String,
    pub email: Option<String>,
    pub host_id: String,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub scopes: Vec<String>,
    pub expires_at: u64,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    pub client_id: String,
    pub subject: String,
    pub email: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostIdentity {
    host_id: String,
}

pub struct Store {
    directory: PathBuf,
}

pub struct StoreLock {
    directory: PathBuf,
    // Dropping the handle releases the OS lock, including during error unwinding.
    _lock: File,
}

impl Store {
    pub fn new() -> Result<Self> {
        let directories = ProjectDirs::from("dev", "Vibe", "Vibe")
            .context("Local authentication directory is unavailable")?;
        Ok(Self {
            directory: directories.data_local_dir().join("auth"),
        })
    }

    pub async fn lock(&self) -> Result<StoreLock> {
        fs::create_dir_all(&self.directory).context("Cannot create authentication directory")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(self.directory.join("auth.lock"))
            .context("Cannot open authentication lock")?;
        let started = Instant::now();
        loop {
            match fs2::FileExt::try_lock_exclusive(&file) {
                Ok(()) => {
                    return Ok(StoreLock {
                        directory: self.directory.clone(),
                        _lock: file,
                    });
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        || error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {}
                Err(_) => bail!("Cannot acquire authentication lock"),
            }
            let remaining = Duration::from_secs(20).saturating_sub(started.elapsed());
            ensure!(
                !remaining.is_zero(),
                "Authentication is busy in another process; try again"
            );
            tokio::time::sleep(remaining.min(Duration::from_millis(100))).await;
        }
    }
}

impl StoreLock {
    pub fn host_id(&self) -> Result<String> {
        let path = self.directory.join("host.json");
        if let Some(bytes) = read_bounded(&path)? {
            let host: HostIdentity = decode(&bytes)?;
            validate_host(&host.host_id)?;
            return Ok(host.host_id);
        }
        let host_id = uuid::Uuid::new_v4().urn().to_string();
        atomic_write(
            &path,
            &encode(&HostIdentity {
                host_id: host_id.clone(),
            })?,
        )?;
        Ok(host_id)
    }

    pub fn credentials(&self) -> Result<Option<CredentialRecord>> {
        let Some(bytes) = read_bounded(&self.directory.join("credentials.bin"))? else {
            return Ok(None);
        };
        let plaintext = unprotect(&bytes)?;
        let record: CredentialRecord = decode(&plaintext)?;
        validate_credentials(&record)?;
        Ok(Some(record))
    }

    pub fn save_credentials(&self, record: &CredentialRecord) -> Result<()> {
        // Corruption must not silently discard an existing rotating refresh token.
        self.credentials()?;
        validate_credentials(record)?;
        let protected = protect(&encode(record)?)?;
        atomic_write(&self.directory.join("credentials.bin"), &protected)
    }

    pub fn registration(&self) -> Result<Option<Registration>> {
        let Some(bytes) = read_bounded(&self.directory.join("registration.json"))? else {
            return Ok(None);
        };
        let registration: Registration = decode(&bytes)?;
        validate_registration(&registration)?;
        Ok(Some(registration))
    }

    pub fn save_registration(&self, registration: &Registration) -> Result<()> {
        self.registration()?;
        validate_registration(registration)?;
        atomic_write(
            &self.directory.join("registration.json"),
            &encode(registration)?,
        )
    }

    pub fn clear_credentials(&self) -> Result<()> {
        match fs::remove_file(self.directory.join("credentials.bin")) {
            Ok(()) => sync_directory(&self.directory),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => bail!("Cannot remove local credentials"),
        }
    }
}

fn validate_host(host_id: &str) -> Result<()> {
    ensure!(
        host_id.starts_with("urn:uuid:") && uuid::Uuid::parse_str(host_id).is_ok(),
        "Invalid local host identity"
    );
    Ok(())
}

fn validate_registration(registration: &Registration) -> Result<()> {
    ensure!(
        !registration.client_id.trim().is_empty() && !registration.subject.trim().is_empty(),
        "Invalid local registration"
    );
    Ok(())
}

fn validate_credentials(record: &CredentialRecord) -> Result<()> {
    ensure!(
        !record.client_id.trim().is_empty()
            && !record.subject.trim().is_empty()
            && !record.id_token.is_empty()
            && !record.access_token.is_empty()
            && !record.refresh_token.is_empty()
            && record.expires_at > 0,
        "Invalid local credentials"
    );
    validate_host(&record.host_id)
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| anyhow::anyhow!("Cannot encode local authentication state"))?;
    ensure!(
        bytes.len() <= MAX_BYTES,
        "Authentication state exceeds 64 KiB limit"
    );
    Ok(bytes)
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    ensure!(
        bytes.len() <= MAX_BYTES,
        "Authentication state exceeds 64 KiB limit"
    );
    serde_json::from_slice(bytes).map_err(|_| anyhow::anyhow!("Invalid local authentication state"))
}

fn read_bounded(path: &Path) -> Result<Option<Vec<u8>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => bail!("Cannot read local authentication state"),
    };
    let metadata = file
        .metadata()
        .context("Cannot inspect local authentication state")?;
    ensure!(
        metadata.is_file() && metadata.len() <= MAX_BYTES as u64,
        "Invalid authentication file size"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "Authentication file permissions must be owner-only"
        );
    }
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .context("Cannot read local authentication state")?;
    ensure!(
        bytes.len() <= MAX_BYTES,
        "Authentication state exceeds 64 KiB limit"
    );
    Ok(Some(bytes))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure!(
        bytes.len() <= MAX_BYTES,
        "Authentication state exceeds 64 KiB limit"
    );
    let parent = path
        .parent()
        .context("Authentication directory is unavailable")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .context("Cannot create authentication temporary file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    temporary
        .write_all(bytes)
        .context("Cannot write local authentication state")?;
    temporary
        .as_file()
        .sync_all()
        .context("Cannot flush local authentication state")?;
    temporary
        .persist(path)
        .map_err(|_| anyhow::anyhow!("Cannot replace local authentication state"))?;
    sync_directory(parent)
}

fn sync_directory(directory: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(directory)?
        .sync_all()
        .context("Cannot flush authentication directory")?;
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

#[cfg(windows)]
fn dpapi(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };
    ensure!(
        bytes.len() <= MAX_BYTES,
        "Authentication state exceeds 64 KiB limit"
    );
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // Input remains alive for the call; DPAPI allocates output with LocalAlloc.
    let success = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if success == 0 {
        bail!("Windows credential protection failed");
    }
    // Always release the DPAPI-owned allocation, including oversized-output errors.
    let result = if output.cbData as usize > MAX_BYTES || output.pbData.is_null() {
        Err(anyhow::anyhow!("Invalid protected credential size"))
    } else {
        // Successful DPAPI output owns cbData readable bytes until LocalFree below.
        Ok(unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec())
    };
    unsafe {
        LocalFree(output.pbData.cast());
    }
    result
}

pub(crate) fn protect(bytes: &[u8]) -> Result<Vec<u8>> {
    #[cfg(windows)]
    {
        dpapi(bytes, true)
    }
    #[cfg(unix)]
    {
        Ok(bytes.to_vec())
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = bytes;
        bail!("Credential storage is unsupported on this platform")
    }
}

pub(crate) fn unprotect(bytes: &[u8]) -> Result<Vec<u8>> {
    #[cfg(windows)]
    {
        dpapi(bytes, false)
    }
    #[cfg(unix)]
    {
        Ok(bytes.to_vec())
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = bytes;
        bail!("Credential storage is unsupported on this platform")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(directory: &Path) -> Store {
        Store {
            directory: directory.join("auth"),
        }
    }

    fn credentials(host_id: String) -> CredentialRecord {
        CredentialRecord {
            client_id: "test-client".into(),
            subject: "test-subject".into(),
            email: Some("test@example.invalid".into()),
            host_id,
            id_token: "synthetic-id-secret".into(),
            access_token: "synthetic-access-secret".into(),
            refresh_token: "synthetic-refresh-secret".into(),
            scopes: vec!["openid".into()],
            expires_at: 12345,
        }
    }

    #[tokio::test]
    async fn protected_roundtrip_and_clear_preserve_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(directory.path());
        let lock = store.lock().await.unwrap();
        let host_id = lock.host_id().unwrap();
        assert_eq!(host_id, lock.host_id().unwrap());
        assert!(lock.credentials().unwrap().is_none());
        let mut record = credentials(host_id.clone());
        let registration = Registration {
            client_id: record.client_id.clone(),
            subject: record.subject.clone(),
            email: record.email.clone(),
        };
        lock.save_registration(&registration).unwrap();
        lock.save_credentials(&record).unwrap();
        record.refresh_token = "synthetic-rotated-secret".into();
        lock.save_credentials(&record).unwrap();
        let restored = lock.credentials().unwrap().unwrap();
        assert_eq!(restored.refresh_token, record.refresh_token);
        assert_eq!(restored.access_token, record.access_token);
        assert_eq!(restored.host_id, host_id);
        assert_eq!(restored.scopes, record.scopes);
        #[cfg(windows)]
        {
            let blob = fs::read(lock.directory.join("credentials.bin")).unwrap();
            assert!(
                !blob
                    .windows(record.access_token.len())
                    .any(|part| part == record.access_token.as_bytes())
            );
            assert!(serde_json::from_slice::<serde_json::Value>(&blob).is_err());
        }
        for file in ["host.json", "registration.json"] {
            let metadata = fs::read_to_string(lock.directory.join(file)).unwrap();
            assert!(!metadata.contains("token"));
            assert!(!metadata.contains("secret"));
        }
        lock.clear_credentials().unwrap();
        lock.clear_credentials().unwrap();
        assert!(lock.credentials().unwrap().is_none());
        assert_eq!(lock.host_id().unwrap(), host_id);
        assert_eq!(
            lock.registration().unwrap().unwrap().client_id,
            registration.client_id
        );
    }

    #[tokio::test]
    async fn corrupt_files_fail_closed_and_are_not_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(directory.path());
        let lock = store.lock().await.unwrap();
        let record = credentials(lock.host_id().unwrap());
        lock.save_credentials(&record).unwrap();
        let path = lock.directory.join("credentials.bin");
        fs::write(&path, b"corrupt-secret-bytes").unwrap();
        let error = lock.credentials().err().unwrap();
        assert!(!format!("{error:#}").contains("corrupt-secret-bytes"));
        assert!(lock.save_credentials(&record).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"corrupt-secret-bytes");
        let registration = Registration {
            client_id: "client".into(),
            subject: "subject".into(),
            email: None,
        };
        lock.save_registration(&registration).unwrap();
        let path = lock.directory.join("registration.json");
        fs::write(&path, b"{corrupt").unwrap();
        assert!(lock.save_registration(&registration).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{corrupt");
        fs::write(lock.directory.join("host.json"), b"{corrupt").unwrap();
        assert!(lock.host_id().is_err());
    }

    #[tokio::test]
    async fn lock_serializes_refresh_and_size_limit_preserves_previous_record() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(directory.path());
        let lock = store.lock().await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(150), store.lock())
                .await
                .is_err()
        );
        let mut record = credentials(lock.host_id().unwrap());
        lock.save_credentials(&record).unwrap();
        record.refresh_token = "x".repeat(MAX_BYTES);
        assert!(lock.save_credentials(&record).is_err());
        assert_eq!(
            lock.credentials().unwrap().unwrap().refresh_token,
            "synthetic-refresh-secret"
        );
        drop(lock);
        let lock = store.lock().await.unwrap();
        let path = lock.directory.join("credentials.bin");
        OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_len(MAX_BYTES as u64 + 1)
            .unwrap();
        assert!(lock.credentials().is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let store = store(directory.path());
        let lock = store.lock().await.unwrap();
        let record = credentials(lock.host_id().unwrap());
        lock.save_credentials(&record).unwrap();
        for file in ["auth.lock", "host.json", "credentials.bin"] {
            assert_eq!(
                fs::metadata(lock.directory.join(file))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}
