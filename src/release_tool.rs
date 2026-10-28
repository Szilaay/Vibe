use anyhow::{Context, Result};
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("init-key") => {
            let public = vibe::updates::init_signing_key()?;
            std::fs::write("release/update-public-key.txt", format!("{public}\n"))?;
            println!("Public update key saved; private key stays in the protected local store.");
        }
        Some("sign") => {
            let path = args.get(2).context("installer path")?;
            let version = args.get(3).context("version")?;
            let notes = args.get(4).map(String::as_str).unwrap_or("VIBE update");
            let signed = vibe::updates::sign_release(std::path::Path::new(path), version, notes)?;
            std::fs::write("dist/latest.json", signed)?;
            println!("Signed manifest saved: dist/latest.json");
        }
        Some("verify") => {
            let manifest = args.get(2).context("manifest path")?;
            let installer = args.get(3).context("installer path")?;
            let release = vibe::updates::verify_manifest(
                &std::fs::read(manifest)?,
                vibe::updates::PUBLIC_KEY,
            )?;
            vibe::updates::verify_file(std::path::Path::new(installer), &release)?;
            println!("Release signature, installer hash and size verified.");
        }
        _ => anyhow::bail!(
            "Usage: vibe-release init-key | sign INSTALLER VERSION NOTES | verify MANIFEST INSTALLER"
        ),
    }
    Ok(())
}
