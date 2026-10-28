//! Native counterpart of the official Antigravity bootstrapper. No installer
//! script is executed, and installation never changes an existing executable.
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use futures_util::{StreamExt, future::BoxFuture, stream::BoxStream};
use serde::Deserialize;
use sha2::{Digest, Sha512};
use subtle::ConstantTimeEq;
use tempfile::NamedTempFile;
use url::Url;

use crate::antigravity::{default_install_directory, is_executable, resolve_executable};

const RELEASE_HOST: &str = "antigravity-cli-auto-updater-974169037036.us-central1.run.app";
const PAYLOAD_PREFIX: &str = "/antigravity-public/antigravity-cli/";
const MAX_MANIFEST: u64 = 64 * 1024;
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;
#[cfg(unix)]
const MAX_EXTRACTED: u64 = 1024 * 1024 * 1024;
const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);
static INSTALL_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Installation status only; callers retain ownership of authentication UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallProgress {
    Checking,
    WaitingForInstaller,
    FetchingManifest,
    Downloading { downloaded: u64, total: Option<u64> },
    Verifying,
    Installing,
    Ready,
}

/// Return an existing CLI or install the official release into its default
/// per-user directory. Dropping this future cancels network work and staging.
pub async fn ensure_installed() -> Result<PathBuf> {
    ensure_installed_with_progress(|_| {}).await
}

pub async fn ensure_installed_with_progress(
    progress: impl Fn(InstallProgress) + Send + Sync,
) -> Result<PathBuf> {
    tokio::time::timeout(INSTALL_TIMEOUT, async {
        progress(InstallProgress::Checking);
        if let Ok(path) = resolve_executable() {
            progress(InstallProgress::Ready);
            return Ok(path);
        }
        progress(InstallProgress::WaitingForInstaller);
        let _guard = INSTALL_LOCK.lock().await;
        if let Ok(path) = resolve_executable() {
            progress(InstallProgress::Ready);
            return Ok(path);
        }
        let directory = default_install_directory()?;
        let name = if cfg!(windows) { "agy.exe" } else { "agy" };
        let target = directory.join(name);
        let source = OfficialSource::new()?;
        let manifest_url = Url::parse(&format!(
            "https://{RELEASE_HOST}/manifests/{}.json",
            release_platform()?
        ))?;
        let result = install_into(&source, &manifest_url, &target, &progress).await?;
        progress(InstallProgress::Ready);
        Ok(result)
    })
    .await
    .context("Az Antigravity CLI telepítése túllépte az 5 perces időkorlátot.")?
}

fn release_platform() -> Result<String> {
    let architecture = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        _ => bail!("Az Antigravity CLI ehhez a processzorhoz nem telepíthető automatikusan."),
    };
    let system = match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "darwin",
        "linux" => "linux",
        _ => bail!(
            "Az Antigravity CLI ehhez az operációs rendszerhez nem telepíthető automatikusan."
        ),
    };
    let musl = system == "linux"
        && (cfg!(target_env = "musl")
            || Path::new("/lib/libc.musl-x86_64.so.1").is_file()
            || Path::new("/lib/libc.musl-aarch64.so.1").is_file());
    Ok(format!(
        "{system}_{architecture}{}",
        if musl { "_musl" } else { "" }
    ))
}

struct Download {
    content_length: Option<u64>,
    chunks: BoxStream<'static, Result<Vec<u8>>>,
}

// The transport boundary lets tests inject byte fixtures without any real
// network request or writes to the user's installation directory.
trait Source: Sync {
    fn get(&self, url: Url) -> BoxFuture<'_, Result<Download>>;
}

struct OfficialSource(reqwest::Client);

impl OfficialSource {
    fn new() -> Result<Self> {
        Ok(Self(
            reqwest::Client::builder()
                .https_only(true)
                // Official URLs are direct; reject redirects entirely rather
                // than let a manifest or CDN redirect widen the trust boundary.
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(180))
                .user_agent(concat!("VIBE/", env!("CARGO_PKG_VERSION")))
                .build()?,
        ))
    }
}

impl Source for OfficialSource {
    fn get(&self, url: Url) -> BoxFuture<'_, Result<Download>> {
        Box::pin(async move {
            let response = self
                .0
                .get(url)
                .send()
                .await
                .context("Az Antigravity letöltési szervere nem érhető el.")?;
            ensure!(
                response.status().is_success(),
                "Az Antigravity letöltési szervere hibát jelzett: {}",
                response.status()
            );
            Ok(Download {
                content_length: response.content_length(),
                chunks: response
                    .bytes_stream()
                    .map(|chunk| {
                        chunk
                            .map(|bytes| bytes.to_vec())
                            .context("Az Antigravity letöltése megszakadt.")
                    })
                    .boxed(),
            })
        })
    }
}

#[derive(Deserialize)]
struct Manifest {
    version: String,
    url: String,
    sha512: String,
}

struct VerifiedManifest {
    url: Url,
    sha512: [u8; 64],
}

fn parse_manifest(bytes: &[u8]) -> Result<VerifiedManifest> {
    let manifest: Manifest =
        serde_json::from_slice(bytes).context("Az Antigravity kiadási jegyzéke érvénytelen.")?;
    ensure!(
        !manifest.version.trim().is_empty() && manifest.version.len() <= 128,
        "Az Antigravity kiadási verziója érvénytelen."
    );
    let url = Url::parse(&manifest.url).context("Érvénytelen Antigravity letöltési cím.")?;
    ensure!(
        url.scheme() == "https"
            && url.host_str() == Some("storage.googleapis.com")
            && url.port_or_known_default() == Some(443)
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path().starts_with(PAYLOAD_PREFIX)
            && !url.path().contains('%'),
        "Az Antigravity letöltési címe nem engedélyezett hivatalos forrás."
    );
    ensure!(
        manifest.sha512.len() == 128 && manifest.sha512.is_ascii(),
        "Az Antigravity SHA-512 ellenőrzőösszege érvénytelen."
    );
    let mut sha512 = [0; 64];
    for (index, pair) in manifest
        .sha512
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .enumerate()
    {
        sha512[index] = u8::from_str_radix(std::str::from_utf8(pair)?, 16)
            .context("Az Antigravity SHA-512 ellenőrzőösszege érvénytelen.")?;
    }
    Ok(VerifiedManifest { url, sha512 })
}

async fn install_into(
    source: &impl Source,
    manifest_url: &Url,
    target: &Path,
    progress: &(impl Fn(InstallProgress) + Send + Sync),
) -> Result<PathBuf> {
    if is_executable(target) {
        return Ok(target.to_owned());
    }
    let directory = target
        .parent()
        .context("Hiányzó Antigravity telepítési könyvtár.")?;
    std::fs::create_dir_all(directory)
        .context("Az Antigravity telepítési könyvtára nem hozható létre.")?;
    let _file_lock = acquire_file_lock(directory).await?;
    if is_executable(target) {
        return Ok(target.to_owned());
    }
    ensure!(
        !target.try_exists()?,
        "Az Antigravity célfájl már létezik, de nem futtatható: {}",
        target.display()
    );

    progress(InstallProgress::FetchingManifest);
    let manifest_download = tokio::time::timeout(Duration::from_secs(20), async {
        let mut download = source.get(manifest_url.clone()).await?;
        ensure!(
            download.content_length.unwrap_or(0) <= MAX_MANIFEST,
            "Túl nagy Antigravity kiadási jegyzék."
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = download.chunks.next().await {
            let chunk = chunk?;
            ensure!(
                bytes.len() as u64 + chunk.len() as u64 <= MAX_MANIFEST,
                "Túl nagy Antigravity kiadási jegyzék."
            );
            bytes.extend_from_slice(&chunk);
        }
        parse_manifest(&bytes)
    })
    .await
    .context("Az Antigravity kiadási jegyzékének letöltése időtúllépés miatt megszakadt.")??;

    let archive = manifest_download.url.path().ends_with(".tar.gz");
    #[cfg(not(unix))]
    ensure!(!archive, "Nem támogatott Antigravity kiadási csomag.");
    let mut download = source.get(manifest_download.url).await?;
    ensure!(
        download.content_length.unwrap_or(0) <= MAX_DOWNLOAD,
        "Túl nagy Antigravity letöltés."
    );
    let mut staging = NamedTempFile::new_in(directory)
        .context("Az Antigravity ideiglenes fájlja nem hozható létre.")?;
    let mut hash = Sha512::new();
    let mut downloaded = 0_u64;
    progress(InstallProgress::Downloading {
        downloaded,
        total: download.content_length,
    });
    while let Some(chunk) = download.chunks.next().await {
        let chunk = chunk?;
        downloaded += chunk.len() as u64;
        ensure!(downloaded <= MAX_DOWNLOAD, "Túl nagy Antigravity letöltés.");
        staging
            .write_all(&chunk)
            .context("Az Antigravity ideiglenes fájlja nem írható.")?;
        hash.update(&chunk);
        progress(InstallProgress::Downloading {
            downloaded,
            total: download.content_length,
        });
    }
    ensure!(downloaded > 0, "Az Antigravity letöltött fájlja üres.");
    if let Some(expected) = download.content_length {
        ensure!(downloaded == expected, "Az Antigravity letöltése hiányos.");
    }
    progress(InstallProgress::Verifying);
    ensure!(
        bool::from(manifest_download.sha512[..].ct_eq(&hash.finalize()[..])),
        "Az Antigravity SHA-512 ellenőrzése sikertelen. A telepítés megszakadt."
    );
    staging.as_file().sync_all()?;
    #[cfg(unix)]
    if archive {
        // This worker only owns temporary files. Cancellation drops its result;
        // it cannot publish an executable after its caller has been canceled.
        let directory = directory.to_owned();
        staging = tokio::task::spawn_blocking(move || extract_binary(staging, &directory))
            .await
            .context("Az Antigravity csomag kibontása megszakadt.")??;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        staging
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o755))?;
    }
    progress(InstallProgress::Installing);
    publish(staging, target)
}

async fn acquire_file_lock(directory: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(".vibe-agy-install.lock"))
        .context("Az Antigravity telepítési zárolása nem nyitható meg.")?;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => return Err(error).context("Az Antigravity telepítése nem zárolható."),
        }
    }
}

fn publish(staging: NamedTempFile, target: &Path) -> Result<PathBuf> {
    // persist_noclobber prevents overwrites even if a different installer creates
    // the final executable after our last existence check.
    match staging.persist_noclobber(target) {
        Ok(file) => {
            file.sync_all()?;
            Ok(target.to_owned())
        }
        Err(error)
            if error.error.kind() == std::io::ErrorKind::AlreadyExists && is_executable(target) =>
        {
            Ok(target.to_owned())
        }
        Err(error) => {
            Err(error.error).context("Az Antigravity CLI végleges fájlja nem hozható létre.")
        }
    }
}

#[cfg(unix)]
fn extract_binary(mut payload: NamedTempFile, directory: &Path) -> Result<NamedTempFile> {
    use std::io::{Read, Seek};
    payload.as_file_mut().rewind()?;
    let decoder = flate2::read::GzDecoder::new(payload.as_file_mut());
    let mut archive = tar::Archive::new(decoder.take(MAX_EXTRACTED + 1));
    for entry in archive
        .entries()
        .context("Érvénytelen Antigravity archívum.")?
    {
        let mut entry = entry.context("Az Antigravity archívuma nem olvasható.")?;
        // Extract exactly the official root binary; never unpack paths, links,
        // permissions, or other archive entries into the filesystem.
        if entry.path()?.as_ref() != Path::new("antigravity") {
            continue;
        }
        ensure!(
            entry.header().entry_type().is_file()
                && entry.size() > 0
                && entry.size() <= MAX_EXTRACTED,
            "Érvénytelen Antigravity bináris az archívumban."
        );
        let expected = entry.size();
        let mut binary = NamedTempFile::new_in(directory)?;
        ensure!(
            std::io::copy(&mut entry, &mut binary)? == expected,
            "Az Antigravity archívuma hiányos."
        );
        binary.as_file().sync_all()?;
        return Ok(binary);
    }
    bail!("Az Antigravity archívuma nem tartalmazza a hivatalos binárist.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct Fixtures {
        manifest: Vec<u8>,
        payload: Vec<u8>,
        requests: Arc<AtomicUsize>,
        pending_payload: bool,
        content_length: Option<u64>,
    }

    impl Fixtures {
        fn new(payload: &[u8]) -> Self {
            let hash = Sha512::digest(payload)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            Self {
                manifest: serde_json::to_vec(&serde_json::json!({
                    "version": "fixture", "url": "https://storage.googleapis.com/antigravity-public/antigravity-cli/fixture/agy", "sha512": hash
                })).unwrap(),
                payload: payload.to_vec(),
                requests: Arc::new(AtomicUsize::new(0)),
                pending_payload: false,
                content_length: None,
            }
        }
    }

    impl Source for Fixtures {
        fn get(&self, url: Url) -> BoxFuture<'_, Result<Download>> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            let manifest = url.host_str() == Some(RELEASE_HOST);
            let content_length = if manifest { None } else { self.content_length };
            let bytes = if manifest {
                self.manifest.clone()
            } else {
                self.payload.clone()
            };
            let chunks = if !manifest && self.pending_payload {
                stream::once(async move { Ok(bytes) })
                    .chain(stream::pending())
                    .boxed()
            } else {
                stream::iter(
                    bytes
                        .chunks(3)
                        .map(|chunk| Ok(chunk.to_vec()))
                        .collect::<Vec<_>>(),
                )
                .boxed()
            };
            Box::pin(async move {
                tokio::task::yield_now().await;
                Ok(Download {
                    content_length,
                    chunks,
                })
            })
        }
    }

    fn manifest_url() -> Url {
        Url::parse(&format!("https://{RELEASE_HOST}/manifests/fixture.json")).unwrap()
    }
    fn target(directory: &Path) -> PathBuf {
        directory.join(if cfg!(windows) { "agy.exe" } else { "agy" })
    }

    #[tokio::test]
    async fn installs_verified_fixture_and_preserves_existing_executable() {
        let directory = tempfile::tempdir().unwrap();
        let target = target(directory.path());
        let fixture = Fixtures::new(b"fixture executable; never executed");
        install_into(&fixture, &manifest_url(), &target, &|_| {})
            .await
            .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), fixture.payload);
        assert!(is_executable(&target));
        let other = Fixtures::new(b"replacement must not be fetched");
        install_into(&other, &manifest_url(), &target, &|_| {})
            .await
            .unwrap();
        assert_eq!(other.requests.load(Ordering::SeqCst), 0);
        assert_eq!(std::fs::read(&target).unwrap(), fixture.payload);
    }

    #[tokio::test]
    async fn checksum_failure_leaves_no_executable_or_staging() {
        let directory = tempfile::tempdir().unwrap();
        let target = target(directory.path());
        let mut fixture = Fixtures::new(b"expected");
        fixture.payload = b"corrupted".to_vec();
        assert!(
            install_into(&fixture, &manifest_url(), &target, &|_| {})
                .await
                .is_err()
        );
        assert!(!target.exists());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1); // lock only
    }

    #[tokio::test]
    async fn oversized_response_rejected_before_staging() {
        let directory = tempfile::tempdir().unwrap();
        let target = target(directory.path());
        let mut fixture = Fixtures::new(b"fixture");
        fixture.content_length = Some(MAX_DOWNLOAD + 1);
        assert!(
            install_into(&fixture, &manifest_url(), &target, &|_| {})
                .await
                .is_err()
        );
        assert!(!target.exists());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn cancellation_cleans_partial_download_and_releases_lock() {
        let directory = tempfile::tempdir().unwrap();
        let target = target(directory.path());
        let mut fixture = Fixtures::new(b"partial");
        fixture.pending_payload = true;
        assert!(
            tokio::time::timeout(
                Duration::from_millis(30),
                install_into(&fixture, &manifest_url(), &target, &|_| {})
            )
            .await
            .is_err()
        );
        assert!(!target.exists());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        install_into(&Fixtures::new(b"retry"), &manifest_url(), &target, &|_| {})
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn simultaneous_installers_download_once() {
        let directory = tempfile::tempdir().unwrap();
        let target = target(directory.path());
        let fixture = Fixtures::new(b"fixture");
        let manifest_url = manifest_url();
        let (first, second) = tokio::join!(
            install_into(&fixture, &manifest_url, &target, &|_| {}),
            install_into(&fixture, &manifest_url, &target, &|_| {})
        );
        first.unwrap();
        second.unwrap();
        assert_eq!(fixture.requests.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn manifest_rejects_untrusted_urls_and_invalid_checksums() {
        let fixture = Fixtures::new(b"fixture");
        let original: serde_json::Value = serde_json::from_slice(&fixture.manifest).unwrap();
        for address in [
            "http://storage.googleapis.com/antigravity-public/antigravity-cli/a",
            "https://storage.googleapis.com.evil.invalid/antigravity-public/antigravity-cli/a",
            "https://storage.googleapis.com/another-bucket/a",
            "https://user@storage.googleapis.com/antigravity-public/antigravity-cli/a",
            "https://storage.googleapis.com/antigravity-public/antigravity-cli/a?redirect=elsewhere",
            "https://storage.googleapis.com/antigravity-public/antigravity-cli/%2e%2e/a",
        ] {
            let mut modified = original.clone();
            modified["url"] = address.into();
            assert!(
                parse_manifest(&serde_json::to_vec(&modified).unwrap()).is_err(),
                "{address}"
            );
        }
        for hash in ["", "abc", &"z".repeat(128)] {
            let mut modified = original.clone();
            modified["sha512"] = hash.into();
            assert!(parse_manifest(&serde_json::to_vec(&modified).unwrap()).is_err());
        }
    }

    #[test]
    fn atomic_publication_never_replaces_existing_target() {
        let directory = tempfile::tempdir().unwrap();
        let target = target(directory.path());
        std::fs::write(&target, b"existing").unwrap();
        let mut staging = NamedTempFile::new_in(directory.path()).unwrap();
        staging.write_all(b"replacement").unwrap();
        let _ = publish(staging, &target);
        assert_eq!(std::fs::read(target).unwrap(), b"existing");
    }

    #[cfg(unix)]
    #[test]
    fn archive_extracts_only_root_binary_and_rejects_links() {
        fn package(entry_type: tar::EntryType) -> NamedTempFile {
            let mut payload = NamedTempFile::new().unwrap();
            {
                let encoder =
                    flate2::write::GzEncoder::new(&mut payload, flate2::Compression::fast());
                let mut archive = tar::Builder::new(encoder);
                let mut header = tar::Header::new_gnu();
                header.set_size(7);
                header.set_mode(0o755);
                header.set_entry_type(entry_type);
                header.set_cksum();
                archive
                    .append_data(&mut header, "antigravity", &b"fixture"[..])
                    .unwrap();
                archive.into_inner().unwrap().finish().unwrap();
            }
            payload
        }
        let directory = tempfile::tempdir().unwrap();
        let binary = extract_binary(package(tar::EntryType::Regular), directory.path()).unwrap();
        assert_eq!(std::fs::read(binary.path()).unwrap(), b"fixture");
        assert!(extract_binary(package(tar::EntryType::Symlink), directory.path()).is_err());
    }
}
