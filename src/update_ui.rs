//! Desktop update workflow. Network work and hashing never run on the UI thread.
use crate::updates::{self, Release};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

enum Event {
    Checked(Result<Option<Release>, String>),
    Downloaded(Result<PathBuf, String>),
}
#[derive(Clone)]
pub struct InstallPlan {
    path: PathBuf,
    release: Release,
}
impl InstallPlan {
    pub fn launch(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.path.parent() == Some(updates::directory()?.as_path()),
            "Invalid installer location"
        );
        anyhow::ensure!(
            std::fs::metadata(&self.path)?.len() == self.release.size,
            "Installer changed after download"
        );
        std::process::Command::new(&self.path).spawn()?;
        Ok(())
    }
}
#[derive(Default)]
pub struct UpdateCenter {
    receiver: Option<mpsc::Receiver<Event>>,
    release: Option<Release>,
    ready: Option<PathBuf>,
    progress: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    checking: bool,
    visible: bool,
    message: String,
    next_check: Option<Instant>,
}
impl UpdateCenter {
    pub fn failure(&mut self, message: String) {
        self.message = message;
        self.visible = true;
    }
    pub fn request_check(&mut self, ctx: &egui::Context, enabled: bool) {
        self.visible = true;
        if !enabled {
            self.message = "Demó módban nincs hálózati frissítéskeresés.".into();
        } else if self.receiver.is_none() {
            self.start_check(ctx);
        }
    }
    fn start_check(&mut self, ctx: &egui::Context) {
        self.next_check = Some(Instant::now() + Duration::from_secs(21600));
        if updates::PUBLIC_KEY.trim().is_empty() {
            self.message = "A frissítési kulcs nincs beállítva ebben a buildben.".into();
            return;
        }
        self.checking = true;
        self.message = "Frissítések keresése…".into();
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        let repaint = ctx.clone();
        if let Ok(root) = updates::directory() {
            let _ = std::fs::create_dir_all(&root);
            let _ = std::fs::write(root.join("last-check"), updates::epoch().to_string());
        }
        std::thread::spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(anyhow::Error::from)
                .and_then(|runtime| runtime.block_on(updates::check()))
                .map_err(|error| format!("{error:#}"));
            let _ = sender.send(Event::Checked(result));
            repaint.request_repaint();
        });
    }
    pub fn show(&mut self, ctx: &egui::Context, automatic: bool) -> Option<InstallPlan> {
        if automatic {
            let due = *self.next_check.get_or_insert_with(|| {
                let last = updates::directory()
                    .ok()
                    .and_then(|root| std::fs::read_to_string(root.join("last-check")).ok())
                    .and_then(|text| text.parse::<u64>().ok())
                    .unwrap_or(0);
                let wait = 21600u64
                    .saturating_sub(updates::epoch().saturating_sub(last))
                    .max(20);
                Instant::now() + Duration::from_secs(wait)
            });
            if Instant::now() >= due && self.receiver.is_none() {
                self.start_check(ctx);
            }
            ctx.request_repaint_after(
                due.saturating_duration_since(Instant::now())
                    .max(Duration::from_secs(1)),
            );
        }
        let event = self
            .receiver
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok());
        if let Some(event) = event {
            self.receiver = None;
            self.checking = false;
            match event {
                Event::Checked(Ok(Some(release))) => {
                    self.release = Some(release);
                    self.ready = None;
                    self.message.clear();
                    self.visible = true;
                }
                Event::Checked(Ok(None)) => {
                    self.release = None;
                    self.ready = None;
                    self.message = "A legfrissebb VIBE-verzió van telepítve.".into();
                }
                Event::Checked(Err(error)) | Event::Downloaded(Err(error)) => {
                    self.message = format!("A frissítés nem sikerült: {error}")
                }
                Event::Downloaded(Ok(path)) => {
                    self.ready = Some(path);
                    self.message = "A telepítő aláírása és ellenőrzőösszege rendben.".into();
                }
            }
        }
        if self.receiver.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        if !self.visible {
            return None;
        }
        let mut open = true;
        let mut install = None;
        let mut start_download = false;
        egui::Window::new("VIBE frissítés")
            .id(egui::Id::new("vibe-updater"))
            .order(egui::Order::Foreground)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(format!("Telepített verzió: {}", env!("CARGO_PKG_VERSION")));
                if let Some(release) = &self.release {
                    ui.heading(format!("Új verzió: {}", release.version));
                    egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .show(ui, |ui| {
                            ui.label(&release.notes);
                        });
                    if self.receiver.is_some() && !self.checking {
                        let fraction =
                            self.progress.load(Ordering::Relaxed) as f32 / release.size as f32;
                        ui.add(egui::ProgressBar::new(fraction).show_percentage());
                        if ui.button("Letöltés megszakítása").clicked() {
                            self.cancel.store(true, Ordering::Relaxed);
                        }
                    } else if let Some(path) = &self.ready {
                        if ui.button("Telepítés és újraindítás").clicked() {
                            install = Some(InstallPlan {
                                path: path.clone(),
                                release: release.clone(),
                            });
                        }
                    } else if self.receiver.is_none()
                        && ui
                            .button(format!(
                                "Telepítő letöltése ({:.0} MB)",
                                release.size as f64 / 1_000_000.0
                            ))
                            .clicked()
                    {
                        start_download = true;
                    }
                    ui.label(
                        egui::RichText::new(
                            "A telepítéshez a VIBE menti és bezárja a munkameneteket.",
                        )
                        .small(),
                    );
                }
                if self.checking {
                    ui.spinner();
                }
                if !self.message.is_empty() {
                    ui.label(&self.message);
                }
            });
        if !open {
            self.visible = false;
        }
        if start_download && let Some(release) = self.release.clone() {
            self.cancel.store(false, Ordering::Relaxed);
            self.progress.store(0, Ordering::Relaxed);
            self.message.clear();
            let (sender, receiver) = mpsc::channel();
            self.receiver = Some(receiver);
            let progress = self.progress.clone();
            let cancel = self.cancel.clone();
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let result = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(anyhow::Error::from)
                    .and_then(|runtime| {
                        runtime.block_on(updates::download(&release, &progress, &cancel))
                    })
                    .map_err(|error| format!("{error:#}"));
                let _ = sender.send(Event::Downloaded(result));
                repaint.request_repaint();
            });
        }
        install
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_never_schedules_network_checks() {
        let ctx = egui::Context::default();
        let mut center = UpdateCenter::default();
        for _ in 0..3 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                assert!(center.show(ctx, false).is_none());
            });
        }
        center.request_check(&ctx, false);
        assert!(center.receiver.is_none());
        assert!(center.next_check.is_none());
    }

    #[test]
    fn new_release_requires_user_action_and_latest_result_clears_stale_download() {
        let ctx = egui::Context::default();
        let mut center = UpdateCenter::default();
        let (sender, receiver) = mpsc::channel();
        center.receiver = Some(receiver);
        sender
            .send(Event::Checked(Ok(Some(Release {
                version: "99.0.0".into(),
                url: "https://github.com/Szilaay/Vibe/releases/download/v99.0.0/VIBE.exe".into(),
                sha256: "0".repeat(64),
                size: 123,
                notes: "Test release".into(),
            }))))
            .unwrap();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            assert!(center.show(ctx, false).is_none());
        });
        assert!(center.visible);
        assert!(center.release.is_some());
        assert!(center.ready.is_none());
        assert!(center.receiver.is_none());
        assert_eq!(center.progress.load(Ordering::Relaxed), 0);

        center.ready = Some(PathBuf::from("stale-installer.exe"));
        let (sender, receiver) = mpsc::channel();
        center.receiver = Some(receiver);
        sender.send(Event::Checked(Ok(None))).unwrap();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            assert!(center.show(ctx, false).is_none());
        });
        assert!(center.release.is_none());
        assert!(center.ready.is_none());
    }
}
