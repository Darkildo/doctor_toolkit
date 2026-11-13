//! Background work. Long operations run on worker threads and report back
//! through a channel, so the interface never blocks.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::thread;

use doctor_toolkit::game::{GameInstance, ResourceIndex, StreamFile};
use doctor_toolkit::navigation::unpack::{self, PackOptions, PackReport};
use doctor_toolkit::resource::names::type_tags;
use doctor_toolkit_mods::i18n::{tr, trf, trn};

/// Results delivered to the interface thread.
pub enum JobResult {
    GameOpened(Result<(GameInstance, ResourceIndex, Vec<String>), String>),
    Extracted {
        written: usize,
        failed: Vec<String>,
        dir: PathBuf,
    },
    /// An unpacked folder was recorded in the workspace.
    Packed(Result<(PackReport, PathBuf), String>),
    /// The job panicked: a bug, logged by the panic hook.
    Failed {
        label: String,
        message: String,
    },
}

/// Progress of a running job, shared with the worker thread.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
    pub cancel: AtomicBool,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            0.0
        } else {
            self.done.load(Ordering::Relaxed) as f32 / total as f32
        }
    }
}

/// A job shown in the status bar.
pub struct Job {
    pub label: String,
    pub progress: Arc<Progress>,
}

fn spawn(
    label: String,
    ctx: &egui::Context,
    tx: &Sender<JobResult>,
    work: impl FnOnce(&Progress) -> JobResult + Send + 'static,
) -> Job {
    let progress = Arc::new(Progress::default());
    let (ctx, tx, shared) = (ctx.clone(), tx.clone(), Arc::clone(&progress));
    let job_label = label.clone();
    thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&shared)))
            .unwrap_or_else(|payload| JobResult::Failed {
                label: job_label,
                message: panic_message(payload.as_ref()),
            });
        // The receiver is gone only when the window was closed.
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    Job { label, progress }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

/// Runs `work` on a worker thread for a view: the result (or the message of
/// a panic) arrives on the returned channel and the window repaints. Not
/// shown in the status bar; for work of a fraction of a second to a few
/// seconds that the view waits for (decoding a sound).
#[cfg_attr(not(feature = "audio"), allow(dead_code))]
pub fn background<T: Send + 'static>(
    ctx: &egui::Context,
    work: impl FnOnce() -> T + Send + 'static,
) -> Receiver<Result<T, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = ctx.clone();
    thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
            .map_err(|payload| panic_message(payload.as_ref()));
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    rx
}

/// Scans a game directory and indexes every resource.
pub fn open_game(dir: PathBuf, ctx: &egui::Context, tx: &Sender<JobResult>) -> Job {
    let label = trf("jobs.indexing", &[("dir", &dir.display())]);
    let repaint = ctx.clone();
    spawn(label, ctx, tx, move |progress| {
        let game = match GameInstance::open(&dir) {
            Ok(game) => game,
            Err(error) => {
                doctor_toolkit::report::log(&format!("cannot open {}: {error}", dir.display()));
                return JobResult::GameOpened(Err(error.to_string()));
            }
        };
        doctor_toolkit::report::log_game(&game);
        progress
            .total
            .store(game.streams().len(), Ordering::Relaxed);
        let (index, errors) = game.build_index(|_| {
            progress.done.fetch_add(1, Ordering::Relaxed);
            repaint.request_repaint();
        });
        let errors = errors
            .into_iter()
            .map(|(i, e)| format!("{}: {e}", game.streams()[i].rel_path))
            .collect();
        JobResult::GameOpened(Ok((game, index, errors)))
    })
}

/// Writes the decompressed data of the given resources of a stream into `dir`.
pub fn extract(
    stream: Arc<StreamFile>,
    resources: Vec<usize>,
    dir: PathBuf,
    ctx: &egui::Context,
    tx: &Sender<JobResult>,
) -> Job {
    let label = trn("jobs.extracting", resources.len() as u64, &[]);
    spawn(label, ctx, tx, move |progress| {
        progress.total.store(resources.len(), Ordering::Relaxed);
        let mut written = 0;
        let mut failed = Vec::new();
        for index in resources {
            if progress.cancel.load(Ordering::Relaxed) {
                failed.push(tr("jobs.cancelled").to_owned());
                break;
            }
            match write_resource(&stream, index, &dir) {
                Ok(()) => written += 1,
                Err(error) => failed.push(trf(
                    "jobs.resource_failed",
                    &[("index", &index), ("error", &error)],
                )),
            }
            progress.done.fetch_add(1, Ordering::Relaxed);
        }
        JobResult::Extracted {
            written,
            failed,
            dir,
        }
    })
}

/// Records the files of an unpacked folder that changed in the workspace of
/// the game, as `doctor-toolkit pack` does.
pub fn pack(
    game: Arc<GameInstance>,
    dir: PathBuf,
    ctx: &egui::Context,
    tx: &Sender<JobResult>,
) -> Job {
    let label = trf("jobs.packing", &[("dir", &dir.display())]);
    spawn(label, ctx, tx, move |_| {
        JobResult::Packed(
            unpack::pack(&game, &dir, &PackOptions::default())
                .map(|report| (report, dir))
                .map_err(|e| e.to_string()),
        )
    })
}

/// File name used for an extracted resource; matches the command line tool.
pub fn resource_file_name(stream: &StreamFile, index: usize) -> String {
    let header = &stream.layout().resources[index].header;
    format!(
        "{index:05}_{}_{:08x}.bin",
        type_tags().display(header.type_id),
        header.ids[3]
    )
}

fn write_resource(stream: &StreamFile, index: usize, dir: &Path) -> Result<(), String> {
    let data = stream.resource_data(index).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(resource_file_name(stream, index));
    std::fs::write(&path, data).map_err(|e| format!("{}: {e}", path.display()))
}
