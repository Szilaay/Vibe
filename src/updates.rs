//! Signed update manifests and bounded, user-approved installer downloads.
use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const REPOSITORY: &str = "Szilaay/Vibe";
pub const PUBLIC_KEY: &str = include_str!("../release/update-public-key.txt");
const MAX_INSTALLER: u64 = 1_500_000_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub notes: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    payload: String,
    signature: String,
}

fn validate_release(release: &Release) -> Result<()> {
    let version = semver::Version::parse(&release.version)?;
    ensure!(
        release.version.len() <= 64 && version.pre.is_empty(),
        "Invalid stable release version"
    );
    ensure!(
        release.size > 0 && release.size <= MAX_INSTALLER,
        "Invalid installer size"
    );
    ensure!(
        release.sha256.len() == 64 && release.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid installer checksum"
    );
    ensure!(release.notes.len() <= 4096, "Release notes too long");
    let url = reqwest::Url::parse(&release.url)?;
    ensure!(
        url.scheme() == "https"
            && url.host_str() == Some("github.com")
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Untrusted update URL"
    );
    ensure!(
        url.path().starts_with(&format!(
            "/{REPOSITORY}/releases/download/v{}/",
            release.version
        )) && url.path().ends_with(".exe"),
        "Installer must belong to this VIBE release"
    );
    Ok(())
}
pub fn verify_manifest(bytes: &[u8], public_key: &str) -> Result<Release> {
    ensure!(bytes.len() <= 65536, "Update manifest too large");
    let envelope: Envelope = serde_json::from_slice(bytes)?;
    let payload = STANDARD.decode(envelope.payload)?;
    ensure!(payload.len() <= 32768, "Update payload too large");
    let key_bytes: [u8; 32] = STANDARD
        .decode(public_key.trim())?
        .try_into()
        .map_err(|_| anyhow::anyhow!("Invalid update public key"))?;
    let key = VerifyingKey::from_bytes(&key_bytes)?;
    let signature = Signature::from_slice(&STANDARD.decode(envelope.signature)?)?;
    key.verify_strict(&payload, &signature)
        .context("Update signature is invalid")?;
    let release: Release = serde_json::from_slice(&payload)?;
    validate_release(&release)?;
    Ok(release)
}
pub fn directory() -> Result<PathBuf> {
    Ok(directories::ProjectDirs::from("dev", "Vibe", "Vibe")
        .context("Missing app directory")?
        .data_local_dir()
        .join("updates"))
}
fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent("VIBE-updater")
        .connect_timeout(std::time::Duration::from_secs(15))
        .read_timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let allowed = attempt.url().scheme() == "https"
                && matches!(
                    attempt.url().host_str(),
                    Some(
                        "github.com"
                            | "release-assets.githubusercontent.com"
                            | "objects.githubusercontent.com"
                    )
                );
            if allowed && attempt.previous().len() < 6 {
                attempt.follow()
            } else {
                attempt.error("Untrusted update redirect")
            }
        }))
        .build()?)
}
pub async fn check() -> Result<Option<Release>> {
    let mut response = client()?
        .get(format!(
            "https://github.com/{REPOSITORY}/releases/latest/download/latest.json"
        ))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?
        .error_for_status()?;
    ensure!(
        response.content_length().is_none_or(|size| size <= 65536),
        "Update manifest too large"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 65536,
            "Update manifest too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    let release = verify_manifest(&bytes, PUBLIC_KEY)?;
    if semver::Version::parse(&release.version)?
        > semver::Version::parse(env!("CARGO_PKG_VERSION"))?
    {
        Ok(Some(release))
    } else {
        Ok(None)
    }
}
pub fn hash_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub fn verify_file(path: &Path, release: &Release) -> Result<()> {
    ensure!(
        fs::metadata(path)?.len() == release.size,
        "Installer size mismatch"
    );
    ensure!(
        hash_file(path)?.eq_ignore_ascii_case(&release.sha256),
        "Installer checksum mismatch"
    );
    Ok(())
}
pub async fn download(
    release: &Release,
    progress: &std::sync::atomic::AtomicU64,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<PathBuf> {
    use std::sync::atomic::Ordering;
    validate_release(release)?;
    let root = directory()?;
    fs::create_dir_all(&root)?;
    let mut file = tempfile::NamedTempFile::new_in(&root)?;
    let mut response = client()?
        .get(&release.url)
        .timeout(std::time::Duration::from_secs(1800))
        .send()
        .await?
        .error_for_status()?;
    if let Some(size) = response.content_length() {
        ensure!(size == release.size, "Unexpected installer size");
    }
    let mut written = 0u64;
    while let Some(chunk) = response.chunk().await? {
        ensure!(!cancel.load(Ordering::Relaxed), "Download cancelled");
        written = written
            .checked_add(chunk.len() as u64)
            .context("Installer too large")?;
        ensure!(written <= release.size, "Installer exceeds signed size");
        file.write_all(&chunk)?;
        progress.store(written, Ordering::Relaxed);
    }
    ensure!(!cancel.load(Ordering::Relaxed), "Download cancelled");
    file.as_file().sync_all()?;
    verify_file(file.path(), release)?;
    let destination = root.join(format!("VIBE-Setup-{}.exe", release.version));
    file.persist(&destination).map_err(|error| error.error)?;
    Ok(destination)
}
pub fn epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// Developer release tooling. The protected private key is never distributed.
fn signing_key_path() -> Result<PathBuf> {
    Ok(directories::ProjectDirs::from("dev", "Vibe", "Vibe")
        .context("Missing release directory")?
        .data_local_dir()
        .join("release-signing")
        .join("ed25519.dpapi"))
}
fn load_signing_key() -> Result<SigningKey> {
    let bytes = fs::read(signing_key_path()?)?;
    ensure!(bytes.len() <= 65536, "Invalid signing key");
    let decoded = crate::auth_store::unprotect(&bytes)?;
    let key: [u8; 32] = decoded
        .try_into()
        .map_err(|_| anyhow::anyhow!("Invalid signing key"))?;
    Ok(SigningKey::from_bytes(&key))
}
pub fn init_signing_key() -> Result<String> {
    let path = signing_key_path()?;
    if !path.exists() {
        use rand::RngCore;
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let protected = crate::auth_store::protect(&bytes)?;
        fs::create_dir_all(path.parent().context("Missing signing key directory")?)?;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        file.write_all(&protected)?;
        file.sync_all()?;
    }
    Ok(STANDARD.encode(load_signing_key()?.verifying_key().to_bytes()))
}
pub fn sign_release(installer: &Path, version: &str, notes: &str) -> Result<Vec<u8>> {
    let release = Release {
        version: version.into(),
        url: format!(
            "https://github.com/{REPOSITORY}/releases/download/v{version}/VIBE-Setup-{version}.exe"
        ),
        sha256: hash_file(installer)?,
        size: fs::metadata(installer)?.len(),
        notes: notes.into(),
    };
    validate_release(&release)?;
    let payload = serde_json::to_vec(&release)?;
    let key = load_signing_key()?;
    ensure!(
        STANDARD.encode(key.verifying_key().to_bytes()) == PUBLIC_KEY.trim(),
        "Signing key does not match the embedded public key"
    );
    let signature = key.sign(&payload);
    Ok(serde_json::to_vec(&Envelope {
        payload: STANDARD.encode(payload),
        signature: STANDARD.encode(signature.to_bytes()),
    })?)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (SigningKey, Release) {
        (
            SigningKey::from_bytes(&[42; 32]),
            Release {
                version: "0.3.0".into(),
                url: format!(
                    "https://github.com/{REPOSITORY}/releases/download/v0.3.0/VIBE-Setup-0.3.0.exe"
                ),
                sha256: "a".repeat(64),
                size: 128,
                notes: "Test release".into(),
            },
        )
    }
    fn envelope(key: &SigningKey, release: &Release) -> Vec<u8> {
        let payload = serde_json::to_vec(release).unwrap();
        serde_json::to_vec(&Envelope {
            payload: STANDARD.encode(&payload),
            signature: STANDARD.encode(key.sign(&payload).to_bytes()),
        })
        .unwrap()
    }
    #[test]
    fn signatures_reject_modified_payload_and_wrong_keys() {
        let (key, release) = fixture();
        let public = STANDARD.encode(key.verifying_key().to_bytes());
        let bytes = envelope(&key, &release);
        assert!(verify_manifest(&bytes, &public).is_ok());
        let wrong = STANDARD.encode(SigningKey::from_bytes(&[43; 32]).verifying_key().to_bytes());
        assert!(verify_manifest(&bytes, &wrong).is_err());
        let mut changed: Envelope = serde_json::from_slice(&bytes).unwrap();
        let mut release = release;
        release.size += 1;
        changed.payload = STANDARD.encode(serde_json::to_vec(&release).unwrap());
        assert!(verify_manifest(&serde_json::to_vec(&changed).unwrap(), &public).is_err());
    }
    #[test]
    fn signed_metadata_still_requires_trusted_versioned_https_urls() {
        let (key, mut release) = fixture();
        let public = STANDARD.encode(key.verifying_key().to_bytes());
        for url in [
            "https://evil.example/setup.exe".to_owned(),
            format!("http://github.com/{REPOSITORY}/releases/download/v0.3.0/setup.exe"),
            format!("https://github.com/{REPOSITORY}/releases/download/v0.2.0/setup.exe"),
        ] {
            release.url = url;
            assert!(verify_manifest(&envelope(&key, &release), &public).is_err());
        }
        assert!(verify_manifest(&vec![0; 65537], &public).is_err());
    }
    #[test]
    fn altered_or_truncated_installer_is_rejected() {
        let (_, mut release) = fixture();
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"installer-fixture").unwrap();
        release.size = 17;
        release.sha256 = hash_file(file.path()).unwrap();
        assert!(verify_file(file.path(), &release).is_ok());
        file.write_all(b"tamper").unwrap();
        assert!(verify_file(file.path(), &release).is_err());
        release.size = 23;
        assert!(verify_file(file.path(), &release).is_err());
    }
}
