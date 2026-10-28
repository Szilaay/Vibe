//! Exercise the same automatic installer used by the Google sign-in button.
//! Does not open sign-in or start a model request.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = vibe::antigravity::ensure_installed_with_progress(|progress| {
        if !matches!(
            progress,
            vibe::antigravity::InstallProgress::Downloading { .. }
        ) {
            eprintln!("{progress:?}");
        }
    })
    .await?;
    println!("Antigravity CLI ready: {}", path.display());
    Ok(())
}
