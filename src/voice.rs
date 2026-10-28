//! Offline dictation. Audio and inference never run on the UI thread.
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui;

use crate::terminal_backend::DictationTarget;

const MAX_SECONDS: u64 = 120;
const RECORDING_TAIL: Duration = Duration::from_millis(200);

// Whisper uses this as a transcript-style vocabulary hint, not an instruction.
// Keep Hungarian as the base language: auto-detection on short mixed utterances
// can select English and lose the surrounding Hungarian words.
const DICTATION_PROMPT: &str = "Nézd meg a chatet. Javítsd a frontend és backend kódját. \
    A betűzött C, L, I rövidítése CLI; A, P, I: API; U, I: UI. \
    A CLI, API, UI, prompt, build, workflow, GitHub, ChatGPT és Codex kifejezéseket használom.";

fn configure_recognition(command: &mut Command) {
    command.args([
        "-l",
        "hu",
        "--prompt",
        DICTATION_PROMPT,
        "--carry-initial-prompt",
        "--beam-size",
        "8",
        "-otxt",
        "-nt",
        "-ng",
    ]);
}

enum Event {
    Recording,
    Processing,
    Finished(Result<String, String>),
}

pub struct VoiceInput {
    events: Option<Receiver<Event>>,
    control: Arc<AtomicU8>, // 0: record, 1: finish, 2: cancel
    target: Option<DictationTarget>,
    recording: bool,
    key: egui::Key,
    learning: bool,
    pub message: String,
    target_label: String,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Default for VoiceInput {
    fn default() -> Self {
        let key = settings_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|name| egui::Key::from_name(name.trim()))
            .unwrap_or(egui::Key::F8);
        Self {
            events: None,
            control: Arc::new(AtomicU8::new(0)),
            target: None,
            recording: false,
            key,
            learning: false,
            message: String::new(),
            target_label: String::new(),
            worker: None,
        }
    }
}

impl VoiceInput {
    pub fn busy(&self) -> bool {
        self.events.is_some()
    }
    pub fn learning(&self) -> bool {
        self.learning
    }

    pub fn cancel(&mut self) {
        self.control.store(2, Ordering::Release);
        self.target = None;
        self.recording = false;
    }

    pub fn shutdown(&mut self) {
        self.cancel();
        // Let the worker close the microphone and reap its inference child
        // before the application exits. Device startup is bounded separately
        // by the OS; do not wait indefinitely for a broken audio driver.
        let deadline = Instant::now() + Duration::from_secs(2);
        while self
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished())
        {
            let _ = self.worker.take().unwrap().join();
        }
    }

    pub fn poll(&mut self) {
        loop {
            let event = self
                .events
                .as_ref()
                .and_then(|events| events.try_recv().ok());
            match event {
                Some(Event::Recording) => {
                    self.recording = true;
                    self.message = format!("Felvétel: {}", self.target_label);
                }
                Some(Event::Processing) => {
                    self.recording = false;
                    self.message = "Beszéd felismerése helyben…".into();
                }
                Some(Event::Finished(result)) => {
                    self.events = None;
                    self.recording = false;
                    let target = self.target.take();
                    if self.control.load(Ordering::Acquire) == 2 {
                        self.message = "Diktálás megszakítva.".into();
                    } else {
                        self.message = match result {
                            Ok(text) => match target
                                .context("A célablak már nem érhető el")
                                .and_then(|target| target.insert(&text))
                            {
                                Ok(()) => {
                                    format!("Beírva: {} · Enterrel küldhető.", self.target_label)
                                }
                                Err(error) => format!("{error} · Felismert szöveg: {text}"),
                            },
                            Err(error) => error,
                        };
                    }
                    break;
                }
                None => break,
            }
        }
    }

    pub fn start(&mut self, target: Result<DictationTarget>, label: String, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let target = match target {
            Ok(target) => target,
            Err(error) => {
                self.message = error.to_string();
                return;
            }
        };
        let assets = match Assets::locate() {
            Ok(assets) => assets,
            Err(error) => {
                self.message = error.to_string();
                return;
            }
        };
        self.control = Arc::new(AtomicU8::new(0));
        self.target = Some(target);
        self.target_label = label;
        self.message = "Mikrofon indítása…".into();
        let (tx, rx) = mpsc::channel();
        self.events = Some(rx);
        let control = self.control.clone();
        let ctx = ctx.clone();
        self.worker = Some(std::thread::spawn(move || {
            let notify = |event| {
                let _ = tx.send(event);
                ctx.request_repaint();
            };
            let result = capture_and_transcribe(assets, &control, &notify)
                .map_err(|error| format!("Diktálás: {error:#}"));
            notify(Event::Finished(result));
        }));
    }

    /// Returns true for a start request; handles stopping and rebinding itself.
    pub fn show(&mut self, ctx: &egui::Context, enabled: bool) -> bool {
        let mut start = false;
        let mut stop = false;
        let learning = self.learning;
        if enabled || self.busy() || learning {
            ctx.input_mut(|input| {
                let shortcut_has_text = input.events.iter().any(|event| {
                    matches!(event, egui::Event::Key { key, modifiers, .. }
                        if (*key == self.key || *key == egui::Key::F35) && modifiers.is_none())
                });
                input.events.retain(|event| {
                    if shortcut_has_text && matches!(event, egui::Event::Text(_)) {
                        return false;
                    }
                    if learning && matches!(event, egui::Event::Text(_) | egui::Event::Paste(_)) {
                        return false;
                    }
                    if let egui::Event::Key {
                        key,
                        pressed,
                        repeat,
                        modifiers,
                        ..
                    } = event
                    {
                        if learning {
                            if *pressed && !repeat {
                                if *key != egui::Key::Escape {
                                    self.key = *key;
                                    self.message = match save_key(self.key) {
                                        Ok(()) => {
                                            format!("Diktálás billentyűje: {}", key_label(self.key))
                                        }
                                        Err(error) => {
                                            format!("A billentyű mentése sikertelen: {error}")
                                        }
                                    };
                                }
                                self.learning = false;
                            }
                            return false;
                        }
                        if (*key == self.key || *key == egui::Key::F35) && modifiers.is_none() {
                            if *pressed && !repeat {
                                if !self.busy() {
                                    start = true;
                                }
                            } else if !*pressed {
                                stop = true;
                            }
                            return false;
                        }
                    }
                    true
                });
            });
        }
        egui::TopBottomPanel::bottom("voice_input_bar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled((enabled && !self.busy()) || self.recording,
                    egui::Button::new(if self.recording { "■ Diktálás vége" } else { "🎤 Diktálás" }))
                    .on_hover_text("Helyi magyar felismerés angol technikai kifejezések támogatásával. Tartsd lenyomva a gyorsbillentyűt (vagy kattints az indításhoz/leállításhoz).")
                    .clicked() {
                        if self.busy() {
                            stop = true;
                        } else {
                            start = true;
                        }
                    }
                ui.add_enabled_ui(!self.busy(), |ui| {
                    let label = if self.key == egui::Key::F35 {
                        "Menü".into()
                    } else {
                        format!("Menü / {}", key_label(self.key))
                    };
                    if ui.button(label).clicked() {
                        self.learning = !self.learning;
                    }
                });
                if self.learning {
                    ui.label("Nyomd meg a kívánt billentyűt; Esc: mégse.");
                    ui.label("Ha a billentyűzeteden nincs Menü gomb, válassz másikat (pl. F8).");
                    if ui.button("Mégse").clicked() { self.learning = false; }
                }
                if self.busy() {
                    ui.spinner();
                    if ui.button("Megszakítás").clicked() {
                        self.cancel();
                        self.message = "Diktálás megszakítása…".into();
                    }
                }
                ui.label(&self.message);
            });
        });
        if (self.recording || self.busy()) && !ctx.input(|input| input.focused) {
            stop = true;
        }
        if stop && self.busy() {
            self.control.store(1, Ordering::Release);
            self.recording = false;
            self.message = "Felvétel lezárása…".into();
            return false;
        }
        start && enabled && !self.learning
    }
}

impl Drop for VoiceInput {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn settings_path() -> Option<PathBuf> {
    crate::storage::workspace_preferences_path()
        .ok()
        .map(|path| path.with_file_name("dictation-key.txt"))
}

fn key_label(key: egui::Key) -> &'static str {
    if key == egui::Key::F35 {
        "Menü"
    } else {
        key.name()
    }
}

fn save_key(key: egui::Key) -> Result<()> {
    let path = settings_path().context("A beállítások könyvtára nem található")?;
    std::fs::create_dir_all(path.parent().context("Hiányzó könyvtár")?)?;
    std::fs::write(path, key.name())?;
    Ok(())
}

struct Assets {
    executable: PathBuf,
    model: PathBuf,
}

impl Assets {
    fn locate() -> Result<Self> {
        let executable = std::env::current_exe()?;
        let mut roots = vec![
            executable
                .parent()
                .context("Hiányzó appkönyvtár")?
                .join("speech"),
        ];
        // Cargo development builds can use the packaged assets in the project root.
        if cfg!(debug_assertions) {
            roots.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("speech"));
        }
        for root in roots {
            let executable = root.join(if cfg!(windows) {
                "whisper-cli.exe"
            } else {
                "whisper-cli"
            });
            let model = root.join("ggml-small.bin");
            if executable.is_file() && model.is_file() {
                return Ok(Self { executable, model });
            }
        }
        bail!(
            "A diktáláshoz hiányzik a speech mappa. A teljes VIBE-windows-x64.zip csomagot bontsd ki, és abból indítsd a VIBE.exe-t. Az exe és a speech mappa maradjon egymás mellett."
        )
    }
}

fn input_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    samples: Arc<Mutex<Vec<f32>>>,
    error: Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = config.channels as usize;
    let limit = config.sample_rate.0 as usize * MAX_SECONDS as usize;
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _| {
            if let Ok(mut samples) = samples.lock() {
                let remaining = limit.saturating_sub(samples.len());
                samples.extend(data.chunks_exact(channels).take(remaining).map(|frame| {
                    frame
                        .iter()
                        .map(|sample| <f32 as cpal::FromSample<T>>::from_sample_(*sample))
                        .sum::<f32>()
                        / channels as f32
                }));
            }
        },
        move |err| {
            if let Ok(mut error) = error.lock() {
                *error = Some(err.to_string());
            }
        },
        None,
    )?)
}

fn capture_and_transcribe(
    assets: Assets,
    control: &AtomicU8,
    notify: &impl Fn(Event),
) -> Result<String> {
    let device = cpal::default_host()
        .default_input_device()
        .context("Nincs elérhető mikrofon. Ellenőrizd a Windows mikrofonengedélyét.")?;
    let supported = device
        .default_input_config()
        .context("A mikrofon nem nyitható meg")?;
    let config: cpal::StreamConfig = supported.clone().into();
    let samples = Arc::new(Mutex::new(Vec::new()));
    let error = Arc::new(Mutex::new(None));
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => {
            input_stream::<f32>(&device, &config, samples.clone(), error.clone())
        }
        cpal::SampleFormat::I16 => {
            input_stream::<i16>(&device, &config, samples.clone(), error.clone())
        }
        cpal::SampleFormat::U16 => {
            input_stream::<u16>(&device, &config, samples.clone(), error.clone())
        }
        format => bail!("Nem támogatott mikrofonformátum: {format:?}"),
    }?;
    stream.play().context("A mikrofon nem indítható")?;
    notify(Event::Recording);
    let started = Instant::now();
    let mut finish_requested = None;
    while started.elapsed() < Duration::from_secs(MAX_SECONDS) {
        match control.load(Ordering::Acquire) {
            2 => break,
            1 => {
                // Keep the last consonant when the shortcut is released while
                // the microphone/OS still has a small amount of buffered audio.
                let finish = finish_requested.get_or_insert_with(Instant::now);
                if finish.elapsed() >= RECORDING_TAIL {
                    break;
                }
            }
            _ => {}
        }
        if let Some(error) = error
            .lock()
            .map_err(|_| anyhow::anyhow!("Mikrofonhiba"))?
            .take()
        {
            bail!(error);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    drop(stream);
    ensure!(control.load(Ordering::Acquire) != 2, "Megszakítva");
    let samples = samples
        .lock()
        .map_err(|_| anyhow::anyhow!("A hangfelvétel nem olvasható"))?;
    ensure!(
        samples.len() >= config.sample_rate.0 as usize / 3,
        "Túl rövid felvétel. Beszélj legalább fél másodpercig."
    );
    let audio = resample(&samples, config.sample_rate.0);
    ensure!(
        audio.iter().any(|sample| sample.abs() > 0.005),
        "Nem hallottam beszédet. Ellenőrizd a mikrofont."
    );
    let temp = tempfile::tempdir()?;
    let wav = temp.path().join("dictation.wav");
    let mut writer = hound::WavWriter::create(
        &wav,
        hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )?;
    for sample in audio {
        writer.write_sample((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    notify(Event::Processing);
    let output = temp.path().join("result");
    let mut command = Command::new(&assets.executable);
    configure_recognition(&mut command);
    command
        .arg("-m")
        .arg(assets.model)
        .arg("-f")
        .arg(wav)
        .arg("-of")
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut child = command
        .spawn()
        .context("A helyi beszédfelismerő nem indítható")?;
    let started = Instant::now();
    loop {
        if control.load(Ordering::Acquire) == 2 || started.elapsed() > Duration::from_secs(300) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("A diktálás megszakadt vagy túllépte az időkorlátot.");
        }
        if let Some(status) = child.try_wait()? {
            ensure!(
                status.success(),
                "A helyi beszédfelismerő hibával leállt ({status})."
            );
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let text = dictation_text(&std::fs::read_to_string(output.with_extension("txt"))?);
    ensure!(!text.is_empty(), "Nem hallottam felismerhető beszédet.");
    Ok(text)
}

fn resample(samples: &[f32], rate: u32) -> Vec<f32> {
    if samples.is_empty() || rate == 0 {
        return Vec::new();
    }
    let length = samples.len() as u64 * 16_000 / u64::from(rate);
    (0..length)
        .map(|index| {
            let position = index as f64 * f64::from(rate) / 16_000.0;
            let left = position as usize;
            let fraction = (position - left as f64) as f32;
            samples[left] * (1.0 - fraction) + samples[(left + 1).min(samples.len() - 1)] * fraction
        })
        .collect()
}

/// Normalize only known spelled acronyms in speech output. Keep this separate
/// from plain_text: ordinary terminal paste must preserve the user's wording.
fn dictation_text(text: &str) -> String {
    let text = plain_text(text);
    let mut words = Vec::new();
    let mut start = None;
    for (index, ch) in text.char_indices() {
        if ch.is_alphanumeric() || ch == '_' {
            start.get_or_insert(index);
        } else if let Some(start) = start.take() {
            words.push((start, index));
        }
    }
    if let Some(start) = start {
        words.push((start, text.len()));
    }
    let mut result = String::new();
    let mut copied = 0;
    let mut index = 0;
    while index < words.len() {
        let mut replacement = None;
        // Phonetic aliases are deliberately limited to the unambiguous CLI
        // sequence; e.g. Hungarian "a" must not be mistaken for the letter A.
        if let Some(triple) = words.get(index..index + 3) {
            let letters: Vec<_> = triple
                .iter()
                .map(|&(start, end)| text[start..end].to_lowercase())
                .collect();
            if matches!(letters[0].as_str(), "c" | "cé" | "szí")
                && matches!(letters[1].as_str(), "l" | "el")
                && matches!(letters[2].as_str(), "i" | "í" | "áj")
            {
                replacement = Some((3, "CLI"));
            }
        }
        for acronym in ["CLI", "API", "UI", "AI", "CSS", "HTML", "SQL", "SDK", "IDE"] {
            if let Some(sequence) = words.get(index..index + acronym.len())
                && sequence
                    .iter()
                    .zip(acronym.chars())
                    .all(|(&(start, end), letter)| text[start..end] == letter.to_string())
            {
                replacement = Some((acronym.len(), acronym));
                break;
            }
        }
        if let Some((count, acronym)) = replacement {
            let sequence = &words[index..index + count];
            // Do not merge across sentences, suffixes, code or other words.
            if sequence.windows(2).all(|pair| {
                text[pair[0].1..pair[1].0]
                    .chars()
                    .all(|ch| ch.is_whitespace() || ch == ',')
            }) {
                result.push_str(&text[copied..sequence[0].0]);
                result.push_str(acronym);
                copied = sequence.last().unwrap().1;
                index += count;
                continue;
            }
        }
        index += 1;
    }
    result.push_str(&text[copied..]);
    result
}

/// Dictation is literal single-line text, never terminal commands or Enter.
pub fn plain_text(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_control() || ch.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spelled_acronyms_are_joined_only_in_dictation() {
        for spelling in ["C, L, I", "C L I", "cé el í", "szí el áj"] {
            assert_eq!(
                dictation_text(&format!("Nézd meg a {spelling} ablakot!")),
                "Nézd meg a CLI ablakot!"
            );
            assert_eq!(plain_text(spelling), spelling);
        }
        assert_eq!(
            dictation_text("Az A, P, I és U, I hibás."),
            "Az API és UI hibás."
        );
    }

    #[test]
    fn dictation_preserves_real_words_sentences_and_identifiers() {
        for text in [
            "Nézd meg! Nézz ide!",
            "a p i",
            "cé elem írás",
            "C. L. I.",
            "C-L-I",
            "C, L, input",
            "foo_C L I",
            "CLI API UI",
            "C, L, I2",
        ] {
            assert_eq!(dictation_text(text), text);
        }
    }

    #[test]
    fn mixed_language_dictation_uses_vocabulary_without_translation() {
        let mut command = Command::new("whisper-cli");
        configure_recognition(&mut command);
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect();
        assert!(args.windows(2).any(|pair| pair == ["-l", "hu"]));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--prompt", DICTATION_PROMPT])
        );
        assert!(args.contains(&"--carry-initial-prompt"));
        assert!(!args.iter().any(|arg| matches!(*arg, "-tr" | "--translate")));
        let text =
            "A frontend layout jó, de a backend API build hibás. Codex, javítsd a workflow-t!";
        assert_eq!(plain_text(text), text);
    }

    fn voice() -> VoiceInput {
        VoiceInput {
            events: None,
            control: Arc::new(AtomicU8::new(0)),
            target: None,
            recording: false,
            key: egui::Key::F8,
            learning: false,
            message: String::new(),
            target_label: "CLI 1".into(),
            worker: None,
        }
    }

    #[test]
    fn menu_and_fallback_are_consumed_before_terminal_input() {
        for key in [egui::Key::F35, egui::Key::F8] {
            let mut voice = voice();
            let ctx = egui::Context::default();
            let _ = ctx.run(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |ctx| {
                    assert!(voice.show(ctx, true));
                    assert!(ctx.input(|input| input.events.is_empty()));
                },
            );
        }
    }

    #[test]
    fn hold_key_records_and_release_stops_dictation() {
        let mut voice = voice();
        let (_tx, rx) = mpsc::channel();
        voice.events = Some(rx);
        voice.recording = true;
        let ctx = egui::Context::default();

        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::F35,
                    physical_key: None,
                    pressed: true,
                    repeat: true,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                assert!(!voice.show(ctx, true));
                assert_eq!(voice.control.load(Ordering::Acquire), 0);
            },
        );

        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::F35,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                assert!(!voice.show(ctx, true));
                assert_eq!(voice.control.load(Ordering::Acquire), 1);
                assert!(!voice.recording);
            },
        );
    }

    #[test]
    fn printable_binding_does_not_type_into_terminal() {
        let mut voice = voice();
        voice.key = egui::Key::V;
        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                events: vec![
                    egui::Event::Key {
                        key: egui::Key::V,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::Text("v".into()),
                ],
                ..Default::default()
            },
            |ctx| {
                assert!(voice.show(ctx, true));
                assert!(ctx.input(|input| input.events.is_empty()));
            },
        );
    }

    #[test]
    fn cancelled_result_is_never_inserted() {
        let mut voice = voice();
        let (tx, rx) = mpsc::channel();
        voice.events = Some(rx);
        voice.cancel();
        tx.send(Event::Finished(Ok("nem küldhető el".into())))
            .unwrap();
        voice.poll();
        assert!(!voice.busy());
        assert_eq!(voice.message, "Diktálás megszakítva.");
    }

    #[test]
    fn dictation_preserves_hungarian_without_terminal_control_or_submit() {
        assert_eq!(
            plain_text("  Árvíztűrő\r\ntükörfúrógép\t\x1b\x00\u{7f}  "),
            "Árvíztűrő tükörfúrógép"
        );
    }

    #[test]
    fn microphone_resamples_common_rates_to_whisper_rate() {
        for rate in [16_000, 44_100, 48_000] {
            let audio = resample(&vec![0.25; rate as usize], rate);
            assert_eq!(audio.len(), 16_000);
            assert!(audio.iter().all(|value| (*value - 0.25).abs() < 0.0001));
        }
        assert_eq!(resample(&[0.0, 1.0], 8_000), vec![0.0, 0.5, 1.0, 1.0]);
    }
}
