//! Download queue ownership, command handling, and bounded transfer execution.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::task::Poll;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_channel::{Receiver, Sender};
use library::{Library, SourceId, Track, TrackId};
use serde::{Deserialize, Serialize};
use sources::{NativeSourceResult, Source, SourceError};
use tracing::warn;

mod track_download;

use track_download::*;

const QUEUE_VERSION: u32 = 1;
const QUEUE_FILE: &str = "queue.json";
const QUEUE_PART_FILE: &str = "queue.json.part";
const RETRY_INTERVAL: Duration = Duration::from_secs(30);
const MAX_ACTIVE_DOWNLOADS: usize = 3;

#[derive(Clone)]
pub struct Downloads {
    root: Arc<PathBuf>,
    commands: Sender<Command>,
}

enum Command {
    Attach {
        source_id: SourceId,
        source: Option<Weak<Source>>,
        loaded: Weak<Library>,
        directory: Option<PathBuf>,
        response: Sender<Result<(), String>>,
    },
    Download {
        source_id: SourceId,
        subject: DownloadSubject,
        quality: DownloadQuality,
        tracks: Vec<Track>,
    },
    ReconcileRule {
        source_id: SourceId,
        rule: DownloadRule,
        quality: DownloadQuality,
        tracks: Vec<Track>,
    },
    Remove {
        source_id: SourceId,
        loaded: Weak<Library>,
        track_ids: Vec<TrackId>,
        notify: bool,
    },
    RemoveRule {
        source_id: SourceId,
        loaded: Option<Weak<Library>>,
        rule: DownloadRule,
        delete_downloads: bool,
    },
    Cancel {
        source_id: SourceId,
        job_id: String,
    },
    ClearJob {
        source_id: SourceId,
        job_id: String,
    },
    SetPaused(bool),
    Move {
        source_id: SourceId,
        job_id: String,
        target_job_id: String,
        after: bool,
    },
    SetDirectory {
        source_id: SourceId,
        directory: Option<PathBuf>,
    },
    Clear {
        source_id: SourceId,
        loaded: Option<Weak<Library>>,
        notify: bool,
    },
}

#[derive(Clone)]
struct AttachedSource {
    source: Option<Weak<Source>>,
    loaded: Weak<Library>,
    directory: Option<PathBuf>,
}

fn same_weak_target<T>(left: &Option<Weak<T>>, right: &Option<Weak<T>>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => Weak::ptr_eq(left, right),
        (None, None) => true,
        _ => false,
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
enum DownloadOwner {
    Subject(DownloadSubject),
    Retained,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DownloadJob {
    id: String,
    subject: DownloadSubject,
    quality: DownloadQuality,
    total_tracks: usize,
    #[serde(default)]
    completed: Vec<TrackId>,
    remaining: Vec<TrackId>,
    state: DownloadQueueState,
}

#[derive(Debug, Deserialize, Serialize)]
struct QueueFile {
    version: u32,
    source_id: SourceId,
    jobs: Vec<DownloadJob>,
}

async fn persist_queue(
    root: &Path,
    source_id: &SourceId,
    jobs: &[DownloadJob],
) -> Result<(), String> {
    let directory = source_directory(root, source_id);
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|error| format!("could not create the download queue directory: {error}"))?;
    let path = directory.join(QUEUE_FILE);
    let part = directory.join(QUEUE_PART_FILE);
    if jobs.is_empty() {
        remove_file_if_present(&part).await?;
        remove_file_if_present(&path).await?;
        return Ok(());
    }
    let encoded = serde_json::to_vec(&QueueFile {
        version: QUEUE_VERSION,
        source_id: source_id.clone(),
        jobs: jobs.to_vec(),
    })
    .map_err(|error| format!("could not encode the download queue: {error}"))?;
    tokio::fs::write(&part, encoded)
        .await
        .map_err(|error| format!("could not save the download queue: {error}"))?;
    tokio::fs::rename(&part, &path)
        .await
        .map_err(|error| format!("could not finish the download queue: {error}"))
}

fn load_queue(root: &Path, source_id: &SourceId) -> Result<Vec<DownloadJob>, String> {
    let directory = source_directory(root, source_id);
    let path = directory.join(QUEUE_FILE);
    let part = directory.join(QUEUE_PART_FILE);
    let (bytes, recovered) = match std::fs::read(&path) {
        Ok(bytes) => (bytes, false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => match std::fs::read(&part) {
            Ok(bytes) => (bytes, true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("could not read {}: {error}", part.display())),
        },
        Err(error) => return Err(format!("could not read {}: {error}", path.display())),
    };
    let queue = serde_json::from_slice::<QueueFile>(&bytes)
        .map_err(|error| format!("could not decode {}: {error}", path.display()))?;
    if queue.version != QUEUE_VERSION || queue.source_id != *source_id {
        return Err("the saved download queue does not match this source".to_string());
    }
    if recovered && let Err(error) = std::fs::rename(&part, &path) {
        warn!(%error, path = %part.display(), "could not finish recovering the download queue");
    }
    Ok(queue.jobs)
}

enum DownloadFailure {
    Item(String),
    Retry(String),
    NeedsAttention(String),
}

struct ActiveDownload {
    source_id: SourceId,
    job_id: String,
    track_id: TrackId,
    subject: DownloadSubject,
    paths: DownloadPaths,
    cancellation: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<Result<(), DownloadFailure>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum DownloadRule {
    EntireLibrary,
    Favorites,
    AllPlaylists,
    LatestFiveAlbums,
}

impl DownloadRule {
    pub const ALL: [Self; 4] = [
        Self::EntireLibrary,
        Self::Favorites,
        Self::AllPlaylists,
        Self::LatestFiveAlbums,
    ];
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum DownloadQuality {
    #[default]
    Original,
    MaxBitrateKbps(u32),
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum DownloadSubject {
    Rule(DownloadRule),
    Track(library::TrackId),
    Album(library::AlbumId),
    Artist(library::ArtistId),
    Genre(library::GenreId),
    Mood(library::MoodId),
    Playlist(library::PlaylistId),
    SmartPlaylist(library::SmartPlaylistId),
    Prepared {
        context_id: String,
        title: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum DownloadQueueState {
    Queued,
    Downloading,
    WaitingForConnection,
    NeedsAttention,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DownloadQueueItem {
    pub id: String,
    pub source_id: SourceId,
    pub subject: DownloadSubject,
    pub quality: DownloadQuality,
    pub completed_tracks: usize,
    pub total_tracks: usize,
    pub state: DownloadQueueState,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DownloadQueueSnapshot {
    pub jobs: Arc<[DownloadQueueItem]>,
    pub downloaded_tracks: usize,
    pub paused: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DownloadFeedbackKind {
    Started,
    Queued,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DownloadFeedback {
    pub subject: DownloadSubject,
    pub item_count: usize,
    pub kind: DownloadFeedbackKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DownloadEvent {
    Queue {
        source_id: SourceId,
        snapshot: Arc<DownloadQueueSnapshot>,
    },
    Feedback(DownloadFeedback),
    Notice(String),
}

struct Actor {
    root: Arc<PathBuf>,
    events: Sender<DownloadEvent>,
    transfers: Arc<TransferClients>,
    attached: HashMap<SourceId, AttachedSource>,
    jobs: HashMap<SourceId, Vec<DownloadJob>>,
    paused: bool,
    next_job: u64,
}

impl Downloads {
    pub fn new(
        root: PathBuf,
        runtime: tokio::runtime::Handle,
        events: Sender<DownloadEvent>,
    ) -> Self {
        let (commands, receiver) = async_channel::unbounded();
        let downloads = Self {
            root: Arc::new(root),
            commands,
        };
        runtime.spawn(run(
            Actor {
                root: Arc::clone(&downloads.root),
                events,
                transfers: Arc::new(TransferClients::default()),
                attached: HashMap::new(),
                jobs: HashMap::new(),
                paused: false,
                next_job: 0,
            },
            receiver,
        ));
        downloads
    }

    pub async fn attach(
        &self,
        source: Option<Arc<Source>>,
        loaded: &Arc<Library>,
        directory: Option<PathBuf>,
    ) -> Result<(), String> {
        let (response, result) = async_channel::bounded(1);
        self.commands
            .send(Command::Attach {
                source_id: loaded.source_id().clone(),
                source: source.as_ref().map(Arc::downgrade),
                loaded: Arc::downgrade(loaded),
                directory,
                response,
            })
            .await
            .map_err(|_| "download operation lane is unavailable".to_string())?;
        result
            .recv()
            .await
            .map_err(|_| "download attachment did not finish".to_string())?
    }

    pub fn download(
        &self,
        source_id: SourceId,
        subject: DownloadSubject,
        quality: DownloadQuality,
        tracks: Vec<Track>,
    ) {
        self.send(Command::Download {
            source_id,
            subject,
            quality,
            tracks,
        });
    }

    pub fn reconcile_rule(
        &self,
        source_id: SourceId,
        rule: DownloadRule,
        quality: DownloadQuality,
        tracks: Vec<Track>,
    ) {
        self.send(Command::ReconcileRule {
            source_id,
            rule,
            quality,
            tracks,
        });
    }

    pub fn remove(
        &self,
        source_id: SourceId,
        loaded: Arc<Library>,
        track_ids: Vec<TrackId>,
        notify: bool,
    ) {
        self.send(Command::Remove {
            source_id,
            loaded: Arc::downgrade(&loaded),
            track_ids,
            notify,
        });
    }

    pub fn remove_rule(
        &self,
        source_id: SourceId,
        loaded: Option<Arc<Library>>,
        rule: DownloadRule,
        delete_downloads: bool,
    ) {
        self.send(Command::RemoveRule {
            source_id,
            loaded: loaded.as_ref().map(Arc::downgrade),
            rule,
            delete_downloads,
        });
    }

    pub fn cancel(&self, source_id: SourceId, job_id: String) {
        self.send(Command::Cancel { source_id, job_id });
    }

    pub fn clear_job(&self, source_id: SourceId, job_id: String) {
        self.send(Command::ClearJob { source_id, job_id });
    }

    pub fn set_paused(&self, paused: bool) {
        self.send(Command::SetPaused(paused));
    }

    pub fn move_job(
        &self,
        source_id: SourceId,
        job_id: String,
        target_job_id: String,
        after: bool,
    ) {
        self.send(Command::Move {
            source_id,
            job_id,
            target_job_id,
            after,
        });
    }

    pub fn set_directory(&self, source_id: SourceId, directory: Option<PathBuf>) {
        self.send(Command::SetDirectory {
            source_id,
            directory,
        });
    }

    pub fn clear(&self, source_id: SourceId, loaded: Option<Arc<Library>>, notify: bool) {
        self.send(Command::Clear {
            source_id,
            loaded: loaded.as_ref().map(Arc::downgrade),
            notify,
        });
    }

    fn send(&self, command: Command) {
        if self.commands.try_send(command).is_err() {
            warn!("download operation lane is unavailable");
        }
    }
}

async fn run(mut actor: Actor, receiver: Receiver<Command>) {
    let mut retry = tokio::time::interval(RETRY_INTERVAL);
    retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut active = Vec::new();
    loop {
        if let Ok(command) = receiver.try_recv() {
            actor.apply(command, &mut active).await;
            continue;
        }
        actor.fill_slots(&mut active).await;
        if !active.is_empty() {
            tokio::select! {
                command = receiver.recv() => {
                    let Ok(command) = command else {
                        actor.abort_matching(&mut active, true, |_| true).await;
                        break;
                    };
                    actor.apply(command, &mut active).await;
                }
                (index, result) = wait_for_finished(&mut active) => {
                    let current = active.swap_remove(index);
                    actor.finish(current, result, &mut active).await;
                }
                _ = retry.tick() => actor.retry_waiting(),
            }
            continue;
        }
        tokio::select! {
            command = receiver.recv() => {
                let Ok(command) = command else {
                    break;
                };
                actor.apply(command, &mut active).await;
            }
            _ = retry.tick() => {
                actor.retry_waiting();
            }
        }
    }
}

async fn wait_for_finished(
    active: &mut [ActiveDownload],
) -> (
    usize,
    Result<Result<(), DownloadFailure>, tokio::task::JoinError>,
) {
    std::future::poll_fn(|context| {
        for (index, download) in active.iter_mut().enumerate() {
            if let Poll::Ready(result) = Pin::new(&mut download.task).poll(context) {
                return Poll::Ready((index, result));
            }
        }
        Poll::Pending
    })
    .await
}

impl Actor {
    async fn apply(&mut self, command: Command, active: &mut Vec<ActiveDownload>) {
        match command {
            Command::Attach {
                source_id,
                source,
                loaded,
                directory,
                response,
            } => {
                let live = loaded.upgrade();
                let directory_changed = self
                    .attached
                    .get(&source_id)
                    .is_some_and(|attached| attached.directory != directory);
                let source_changed = self
                    .attached
                    .get(&source_id)
                    .is_some_and(|attached| !same_weak_target(&attached.source, &source));
                self.discard_matching(active, !directory_changed, |download| {
                    download.source_id == source_id
                        && (source_changed
                            || directory_changed
                            || live.as_ref().is_none_or(|live| {
                                matches!(live.track(&download.track_id), Ok(None))
                            }))
                })
                .await;
                let result = self.attach(source_id, source, loaded, directory).await;
                let _ = response.send(result).await;
            }
            Command::Download {
                source_id,
                subject,
                quality,
                tracks,
            } => {
                self.enqueue(source_id, subject, quality, tracks).await;
            }
            Command::ReconcileRule {
                source_id,
                rule,
                quality,
                tracks,
            } => {
                let desired = tracks
                    .iter()
                    .map(|track| track.id.clone())
                    .collect::<HashSet<_>>();
                let subject = DownloadSubject::Rule(rule);
                let quality_changed = self.jobs.get(&source_id).is_some_and(|jobs| {
                    jobs.iter()
                        .any(|job| job.subject == subject && job.quality != quality)
                });
                self.abort_matching(active, true, |download| {
                    download.source_id == source_id
                        && download.subject == subject
                        && (quality_changed || !desired.contains(&download.track_id))
                })
                .await;
                let active_job_id = active
                    .iter()
                    .find(|download| {
                        download.source_id == source_id
                            && download.subject == DownloadSubject::Rule(rule)
                    })
                    .map(|download| download.job_id.clone());
                self.reconcile_rule(source_id, rule, quality, tracks, active_job_id.as_deref())
                    .await;
            }
            Command::Remove {
                source_id,
                loaded,
                track_ids,
                notify,
            } => {
                let remove = track_ids.iter().collect::<HashSet<_>>();
                self.abort_matching(active, false, |download| {
                    download.source_id == source_id && remove.contains(&download.track_id)
                })
                .await;
                self.force_remove(&source_id, &loaded, track_ids, notify)
                    .await;
            }
            Command::RemoveRule {
                source_id,
                loaded,
                rule,
                delete_downloads,
            } => {
                self.abort_matching(active, true, |download| {
                    download.source_id == source_id
                        && download.subject == DownloadSubject::Rule(rule)
                })
                .await;
                self.remove_rule(&source_id, loaded.as_ref(), rule, delete_downloads)
                    .await;
            }
            Command::Cancel { source_id, job_id } => {
                self.cancel(&source_id, &job_id, active).await;
            }
            Command::ClearJob { source_id, job_id } => {
                self.clear_job(&source_id, &job_id, active).await;
            }
            Command::SetPaused(paused) => {
                if self.paused == paused {
                    return;
                }
                self.paused = paused;
                if paused {
                    self.abort_matching(active, true, |_| true).await;
                }
                self.publish_all().await;
            }
            Command::Move {
                source_id,
                job_id,
                target_job_id,
                after,
            } => {
                self.move_job(&source_id, &job_id, &target_job_id, after)
                    .await;
            }
            Command::SetDirectory {
                source_id,
                directory,
            } => {
                self.abort_matching(active, false, |download| download.source_id == source_id)
                    .await;
                self.discard_previous_directory(&source_id, &directory)
                    .await;
                if let Some(attached) = self.attached.get_mut(&source_id) {
                    attached.directory = directory;
                }
            }
            Command::Clear {
                source_id,
                loaded,
                notify,
            } => {
                self.abort_matching(active, false, |download| download.source_id == source_id)
                    .await;
                self.clear(&source_id, loaded.as_ref(), notify).await;
            }
        }
    }

    async fn attach(
        &mut self,
        source_id: SourceId,
        source: Option<Weak<Source>>,
        loaded: Weak<Library>,
        directory: Option<PathBuf>,
    ) -> Result<(), String> {
        let Some(live) = loaded.upgrade() else {
            self.attached.remove(&source_id);
            return Err("the accepted library is no longer available".to_string());
        };
        let unchanged = self.attached.get(&source_id).is_some_and(|attached| {
            attached.directory == directory
                && Weak::ptr_eq(&attached.loaded, &loaded)
                && same_weak_target(&attached.source, &source)
        });
        self.discard_previous_directory(&source_id, &directory)
            .await;
        self.attached.insert(
            source_id.clone(),
            AttachedSource {
                source: source.clone(),
                loaded: loaded.clone(),
                directory: directory.clone(),
            },
        );
        if unchanged && self.jobs.contains_key(&source_id) {
            self.retry_waiting();
            self.publish(&source_id).await;
            return Ok(());
        }
        let mut attachment_error = None;
        let root = Arc::clone(&self.root);
        let attached_loaded = Arc::clone(&live);
        match tokio::task::spawn_blocking(move || attach_downloaded_files(&root, &attached_loaded))
            .await
        {
            Ok(Ok(stale)) => {
                for paths in stale {
                    if let Err(error) = remove_download_files(&paths).await {
                        attachment_error.get_or_insert(error);
                    }
                }
            }
            Ok(Err(error)) => attachment_error = Some(error),
            Err(error) => {
                attachment_error = Some(format!("download attachment task failed: {error}"));
            }
        }
        let source_available = source.as_ref().and_then(Weak::upgrade).is_some();
        let mut jobs = if let Some(jobs) = self.jobs.get(&source_id) {
            jobs.clone()
        } else {
            match load_queue(&self.root, &source_id) {
                Ok(jobs) => jobs,
                Err(error) => {
                    warn!(%error, %source_id, "could not load the download queue");
                    return Err(error);
                }
            }
        };
        jobs.retain_mut(|job| {
            job.remaining
                .retain(|track_id| live.track(track_id).ok().flatten().is_some());
            job.state = if source_available {
                DownloadQueueState::Queued
            } else {
                DownloadQueueState::WaitingForConnection
            };
            !job.remaining.is_empty()
        });
        let queued_tracks = jobs
            .iter()
            .flat_map(|job| job.remaining.iter().cloned())
            .collect::<HashSet<_>>();
        if let Err(error) =
            cleanup_staging(&self.root, &source_id, directory.as_deref(), &queued_tracks).await
        {
            attachment_error.get_or_insert_with(|| error.to_string());
        }
        drop(live);
        self.jobs.insert(source_id.clone(), jobs);
        self.persist_and_publish(&source_id).await;
        attachment_error.map_or(Ok(()), Err)
    }

    async fn enqueue(
        &mut self,
        source_id: SourceId,
        subject: DownloadSubject,
        quality: DownloadQuality,
        tracks: Vec<Track>,
    ) {
        let Some(attached) = self.attached.get(&source_id) else {
            warn!(%source_id, "ignored a download for an unattached source");
            return;
        };
        let source_available = attached.source.as_ref().and_then(Weak::upgrade).is_some();
        let can_start = !self.paused && source_available;

        let mut seen = HashSet::new();
        let track_ids = tracks
            .into_iter()
            .map(|track| track.id.clone())
            .filter(|track_id| seen.insert(track_id.clone()))
            .collect::<Vec<_>>();
        if track_ids.is_empty() {
            return;
        }

        let owner = DownloadOwner::Subject(subject.clone());
        let mut completed = Vec::new();
        let mut remaining = Vec::new();
        for track_id in &track_ids {
            match add_owner_to_existing_download(&self.root, &source_id, track_id, &owner).await {
                Ok(true) => completed.push(track_id.clone()),
                Ok(false) => remaining.push(track_id.clone()),
                Err(error) => {
                    warn!(%error, %source_id, %track_id, "could not update download ownership");
                    remaining.push(track_id.clone());
                }
            }
        }

        let mut scheduled_tracks = 0usize;
        if !remaining.is_empty() || !completed.is_empty() {
            let jobs = self.jobs.entry(source_id.clone()).or_default();
            if let Some(existing_index) = jobs
                .iter()
                .position(|job| job.subject == subject && job.quality == quality)
            {
                let existing = &mut jobs[existing_index];
                let completed_now = completed.iter().cloned().collect::<HashSet<_>>();
                existing
                    .remaining
                    .retain(|track_id| !completed_now.contains(track_id));
                let mut known = existing
                    .completed
                    .iter()
                    .chain(&existing.remaining)
                    .cloned()
                    .collect::<HashSet<_>>();
                existing.completed.extend(
                    completed
                        .into_iter()
                        .filter(|track_id| known.insert(track_id.clone())),
                );
                let additions = remaining
                    .into_iter()
                    .filter(|track_id| known.insert(track_id.clone()))
                    .collect::<Vec<_>>();
                scheduled_tracks = additions.len();
                existing.remaining.extend(additions);
                existing.total_tracks = existing.completed.len() + existing.remaining.len();
                if existing.state != DownloadQueueState::Downloading {
                    existing.state = if source_available {
                        DownloadQueueState::Queued
                    } else {
                        DownloadQueueState::WaitingForConnection
                    };
                }
                if existing.remaining.is_empty() {
                    jobs.remove(existing_index);
                }
            } else if !remaining.is_empty() {
                scheduled_tracks = remaining.len();
                self.next_job = self.next_job.wrapping_add(1);
                jobs.push(DownloadJob {
                    id: job_id(&source_id, &subject, self.next_job),
                    subject: subject.clone(),
                    quality,
                    total_tracks: track_ids.len(),
                    completed,
                    remaining,
                    state: if source_available {
                        DownloadQueueState::Queued
                    } else {
                        DownloadQueueState::WaitingForConnection
                    },
                });
            }
        }

        self.persist_and_publish(&source_id).await;
        if scheduled_tracks > 0 {
            let _ = self
                .events
                .send(DownloadEvent::Feedback(DownloadFeedback {
                    subject,
                    item_count: scheduled_tracks,
                    kind: if can_start {
                        DownloadFeedbackKind::Started
                    } else {
                        DownloadFeedbackKind::Queued
                    },
                }))
                .await;
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn reconcile_rule(
        &mut self,
        source_id: SourceId,
        rule: DownloadRule,
        quality: DownloadQuality,
        tracks: Vec<Track>,
        active_job_id: Option<&str>,
    ) {
        let Some(attached) = self.attached.get(&source_id).cloned() else {
            warn!(%source_id, "ignored download reconciliation for an unattached source");
            return;
        };
        let source_available = attached.source.as_ref().and_then(Weak::upgrade).is_some();
        let can_start = !self.paused && source_available;

        let mut seen = HashSet::new();
        let track_ids = tracks
            .into_iter()
            .map(|track| track.id.clone())
            .filter(|track_id| seen.insert(track_id.clone()))
            .collect::<Vec<_>>();
        let desired = track_ids.iter().cloned().collect::<HashSet<_>>();
        let subject = DownloadSubject::Rule(rule);
        let owner = DownloadOwner::Subject(subject.clone());

        let records = match load_download_records(&self.root, &source_id) {
            Ok(records) => records,
            Err(error) => {
                warn!(%error, %source_id, "could not read rule downloads");
                return;
            }
        };
        for (track_id, mut record) in records {
            if desired.contains(&track_id) || !record.owners.remove(&owner) {
                continue;
            }
            record.owners.extend(self.queued_owners_for_track(
                &source_id,
                &track_id,
                Some(&subject),
            ));
            let paths = record_download_paths(&self.root, &source_id, &record);
            if record.owners.is_empty() {
                if let Err(error) = remove_download_files(&paths).await {
                    warn!(%error, %source_id, %track_id, "could not remove stale rule download");
                    continue;
                }
                if let Some(loaded) = attached.loaded.upgrade() {
                    let _ = loaded.remove_downloaded_file(&track_id);
                }
            } else if let Err(error) = write_record(&paths, &record).await {
                warn!(%error, %source_id, %track_id, "could not update rule ownership");
            }
        }

        let mut completed = Vec::new();
        let mut remaining = Vec::new();
        for track_id in &track_ids {
            match add_owner_to_existing_download(&self.root, &source_id, track_id, &owner).await {
                Ok(true) => completed.push(track_id.clone()),
                Ok(false) => remaining.push(track_id.clone()),
                Err(error) => {
                    warn!(%error, %source_id, %track_id, "could not update download ownership");
                    remaining.push(track_id.clone());
                }
            }
        }

        let jobs = self.jobs.entry(source_id.clone()).or_default();
        let existing_index = jobs.iter().position(|job| job.subject == subject);
        let existing_id = existing_index.map(|index| jobs[index].id.clone());
        let quality_changed = jobs
            .iter()
            .any(|job| job.subject == subject && job.quality != quality);
        let old_remaining = jobs
            .iter()
            .filter(|job| job.subject == subject)
            .flat_map(|job| job.remaining.iter().cloned())
            .collect::<HashSet<_>>();
        jobs.retain(|job| job.subject != subject);

        let scheduled_tracks = remaining
            .iter()
            .filter(|track_id| quality_changed || !old_remaining.contains(*track_id))
            .count();
        if !remaining.is_empty() {
            let id = existing_id.unwrap_or_else(|| {
                self.next_job = self.next_job.wrapping_add(1);
                job_id(&source_id, &subject, self.next_job)
            });
            let state = if active_job_id == Some(id.as_str()) {
                DownloadQueueState::Downloading
            } else if source_available {
                DownloadQueueState::Queued
            } else {
                DownloadQueueState::WaitingForConnection
            };
            let job = DownloadJob {
                id,
                subject: subject.clone(),
                quality,
                total_tracks: track_ids.len(),
                completed,
                remaining,
                state,
            };
            jobs.insert(existing_index.unwrap_or(jobs.len()).min(jobs.len()), job);
        }
        self.reconcile_staging(&source_id).await;

        self.persist_and_publish(&source_id).await;
        if scheduled_tracks > 0 {
            let _ = self
                .events
                .send(DownloadEvent::Feedback(DownloadFeedback {
                    subject,
                    item_count: scheduled_tracks,
                    kind: if can_start {
                        DownloadFeedbackKind::Started
                    } else {
                        DownloadFeedbackKind::Queued
                    },
                }))
                .await;
        }
    }

    async fn fill_slots(&mut self, active: &mut Vec<ActiveDownload>) {
        if self.paused {
            return;
        }
        while active.len() < MAX_ACTIVE_DOWNLOADS {
            let Some(download) = self.start_next(active).await else {
                break;
            };
            active.push(download);
        }
    }

    async fn start_next(&mut self, active: &[ActiveDownload]) -> Option<ActiveDownload> {
        loop {
            let (source_id, job_id, subject, quality, track_id, state) =
                self.next_candidate(active)?;
            let Some(attached) = self.attached.get(&source_id).cloned() else {
                self.jobs.remove(&source_id);
                self.persist_and_publish(&source_id).await;
                continue;
            };
            let Some(loaded) = attached.loaded.upgrade() else {
                if let Some(job) = self.find_job_mut(&source_id, &job_id) {
                    job.state = DownloadQueueState::WaitingForConnection;
                }
                self.persist_and_publish(&source_id).await;
                continue;
            };
            let Some(source) = attached.source.as_ref().and_then(Weak::upgrade) else {
                if let Some(job) = self.find_job_mut(&source_id, &job_id) {
                    job.state = DownloadQueueState::WaitingForConnection;
                }
                self.persist_and_publish(&source_id).await;
                continue;
            };
            let track = loaded.track(&track_id).ok().flatten();
            drop(loaded);
            let Some(track) = track else {
                self.remove_job_track(&source_id, &job_id, &track_id, false);
                self.persist_and_publish(&source_id).await;
                continue;
            };
            let owner = DownloadOwner::Subject(subject.clone());
            match add_owner_to_existing_download(&self.root, &source_id, &track_id, &owner).await {
                Ok(true) => {
                    self.remove_job_track(&source_id, &job_id, &track_id, true);
                    self.persist_and_publish(&source_id).await;
                    continue;
                }
                Ok(false) => {}
                Err(error) => {
                    warn!(%error, %source_id, %track_id, "could not update download ownership");
                    if let Some(job) = self.find_job_mut(&source_id, &job_id) {
                        job.state = DownloadQueueState::NeedsAttention;
                    }
                    self.persist_and_publish(&source_id).await;
                    continue;
                }
            }
            let paths = new_download_paths(
                &self.root,
                &source_id,
                &track,
                attached.directory.as_deref(),
                quality,
            );
            let entering_download = state != DownloadQueueState::Downloading;
            if let Some(job) = self.find_job_mut(&source_id, &job_id) {
                job.state = DownloadQueueState::Downloading;
            }
            if entering_download {
                self.persist_and_publish(&source_id).await;
            } else {
                self.publish(&source_id).await;
            }
            let task_paths = paths.clone();
            let transfers = Arc::clone(&self.transfers);
            let (cancellation, cancelled) = tokio::sync::oneshot::channel();
            let task = tokio::spawn(download_track(
                source,
                track_id.clone(),
                quality,
                task_paths,
                transfers,
                cancelled,
            ));
            return Some(ActiveDownload {
                source_id,
                job_id,
                track_id,
                subject,
                paths,
                cancellation: Some(cancellation),
                task,
            });
        }
    }

    fn next_candidate(
        &self,
        active: &[ActiveDownload],
    ) -> Option<(
        SourceId,
        String,
        DownloadSubject,
        DownloadQuality,
        TrackId,
        DownloadQueueState,
    )> {
        for (source_id, jobs) in &self.jobs {
            for job in jobs {
                if !matches!(
                    job.state,
                    DownloadQueueState::Queued | DownloadQueueState::Downloading
                ) {
                    break;
                }
                let track_id = job.remaining.iter().find(|track_id| {
                    !active.iter().any(|download| {
                        download.source_id == *source_id && &download.track_id == *track_id
                    })
                });
                if let Some(track_id) = track_id {
                    return Some((
                        source_id.clone(),
                        job.id.clone(),
                        job.subject.clone(),
                        job.quality,
                        track_id.clone(),
                        job.state,
                    ));
                }
            }
        }
        None
    }

    async fn finish(
        &mut self,
        active: ActiveDownload,
        joined: Result<Result<(), DownloadFailure>, tokio::task::JoinError>,
        remaining_active: &mut Vec<ActiveDownload>,
    ) {
        let ActiveDownload {
            source_id,
            job_id,
            track_id,
            subject,
            paths,
            ..
        } = active;
        let result = match joined {
            Ok(Ok(())) => {
                self.commit_transfer(&source_id, &track_id, &subject, &paths)
                    .await
            }
            Ok(Err(error)) => Err(error),
            Err(error) => Err(DownloadFailure::NeedsAttention(format!(
                "download task failed: {error}"
            ))),
        };
        if self.find_job_mut(&source_id, &job_id).is_none() {
            return;
        }
        match result {
            Ok(()) => {
                self.remove_job_track(&source_id, &job_id, &track_id, true);
            }
            Err(DownloadFailure::Item(error)) => {
                warn!(%error, %source_id, %track_id, "could not download track");
                self.remove_job_track(&source_id, &job_id, &track_id, false);
            }
            Err(DownloadFailure::Retry(error)) => {
                warn!(%error, %source_id, "download is waiting for the server");
                self.abort_matching(remaining_active, true, |download| {
                    download.source_id == source_id && download.job_id == job_id
                })
                .await;
                if let Some(job) = self.find_job_mut(&source_id, &job_id) {
                    job.state = DownloadQueueState::WaitingForConnection;
                }
            }
            Err(DownloadFailure::NeedsAttention(error)) => {
                warn!(%error, %source_id, "download needs attention");
                self.abort_matching(remaining_active, true, |download| {
                    download.source_id == source_id && download.job_id == job_id
                })
                .await;
                if let Some(job) = self.find_job_mut(&source_id, &job_id) {
                    job.state = DownloadQueueState::NeedsAttention;
                }
            }
        }
        self.persist_and_publish(&source_id).await;
    }

    async fn commit_transfer(
        &self,
        source_id: &SourceId,
        track_id: &TrackId,
        subject: &DownloadSubject,
        paths: &DownloadPaths,
    ) -> Result<(), DownloadFailure> {
        finalize_download(
            paths,
            source_id.clone(),
            track_id.clone(),
            DownloadOwner::Subject(subject.clone()),
        )
        .await
        .map_err(DownloadFailure::NeedsAttention)?;
        if let Some(loaded) = self
            .attached
            .get(source_id)
            .and_then(|attached| attached.loaded.upgrade())
        {
            loaded
                .set_downloaded_file(track_id.clone(), paths.audio.clone())
                .map_err(|error| DownloadFailure::NeedsAttention(error.to_string()))?;
        }
        Ok(())
    }

    async fn abort_matching(
        &mut self,
        active: &mut Vec<ActiveDownload>,
        preserve: bool,
        matches: impl Fn(&ActiveDownload) -> bool,
    ) -> Vec<TrackId> {
        self.settle_matching(active, preserve, true, matches).await
    }

    async fn discard_matching(
        &mut self,
        active: &mut Vec<ActiveDownload>,
        preserve: bool,
        matches: impl Fn(&ActiveDownload) -> bool,
    ) {
        self.settle_matching(active, preserve, false, matches).await;
    }

    async fn settle_matching(
        &mut self,
        active: &mut Vec<ActiveDownload>,
        preserve: bool,
        commit_completed: bool,
        matches: impl Fn(&ActiveDownload) -> bool,
    ) -> Vec<TrackId> {
        let mut settling = Vec::new();
        let mut index = 0;
        while index < active.len() {
            if !matches(&active[index]) {
                index += 1;
                continue;
            }
            let mut download = active.swap_remove(index);
            if let Some(cancellation) = download.cancellation.take() {
                let _ = cancellation.send(());
            }
            settling.push(download);
        }

        let mut affected = HashSet::new();
        let mut completed_tracks = Vec::new();
        for download in settling {
            let joined = download.task.await;
            let completed = matches!(joined, Ok(Ok(())));
            if completed && commit_completed {
                match self
                    .commit_transfer(
                        &download.source_id,
                        &download.track_id,
                        &download.subject,
                        &download.paths,
                    )
                    .await
                {
                    Ok(()) => {
                        completed_tracks.push(download.track_id.clone());
                        self.remove_job_track(
                            &download.source_id,
                            &download.job_id,
                            &download.track_id,
                            true,
                        );
                    }
                    Err(DownloadFailure::NeedsAttention(error)) => {
                        warn!(
                            %error,
                            source_id = %download.source_id,
                            track_id = %download.track_id,
                            "could not finish a completed download"
                        );
                        if let Some(job) = self.find_job_mut(&download.source_id, &download.job_id)
                        {
                            job.state = DownloadQueueState::NeedsAttention;
                        }
                    }
                    Err(_) => unreachable!("download commit failures need attention"),
                }
                continue;
            }
            affected.insert((download.source_id.clone(), download.job_id.clone()));
            if !preserve && let Err(error) = discard_staging(&download.paths).await {
                warn!(
                    %error,
                    source_id = %download.source_id,
                    track_id = %download.track_id,
                    "could not settle an interrupted download"
                );
            }
        }
        for (source_id, job_id) in affected {
            let still_active = active
                .iter()
                .any(|download| download.source_id == source_id && download.job_id == job_id);
            if let Some(job) = self.find_job_mut(&source_id, &job_id) {
                job.state = if still_active {
                    DownloadQueueState::Downloading
                } else {
                    DownloadQueueState::Queued
                };
            }
        }
        completed_tracks
    }

    async fn reconcile_staging(&self, source_id: &SourceId) {
        let queued = self
            .jobs
            .get(source_id)
            .into_iter()
            .flatten()
            .flat_map(|job| job.remaining.iter().cloned())
            .collect::<HashSet<_>>();
        let directory = self
            .attached
            .get(source_id)
            .and_then(|attached| attached.directory.as_deref());
        if let Err(error) = cleanup_staging(&self.root, source_id, directory, &queued).await {
            warn!(%error, %source_id, "could not reconcile download staging");
        }
    }

    async fn discard_previous_directory(&self, source_id: &SourceId, directory: &Option<PathBuf>) {
        let Some(attached) = self.attached.get(source_id) else {
            return;
        };
        if attached.directory == *directory {
            return;
        }
        if let Err(error) = cleanup_staging(
            &self.root,
            source_id,
            attached.directory.as_deref(),
            &HashSet::new(),
        )
        .await
        {
            warn!(%error, %source_id, "could not remove download staging from the previous folder");
        }
    }

    fn queued_owners_for_track(
        &self,
        source_id: &SourceId,
        track_id: &TrackId,
        excluded_subject: Option<&DownloadSubject>,
    ) -> HashSet<DownloadOwner> {
        self.jobs
            .get(source_id)
            .into_iter()
            .flatten()
            .filter(|job| excluded_subject != Some(&job.subject))
            .filter(|job| job.remaining.contains(track_id))
            .map(|job| DownloadOwner::Subject(job.subject.clone()))
            .collect()
    }

    fn find_job_mut(&mut self, source_id: &SourceId, job_id: &str) -> Option<&mut DownloadJob> {
        self.jobs
            .get_mut(source_id)?
            .iter_mut()
            .find(|job| job.id == job_id)
    }

    fn remove_job_track(
        &mut self,
        source_id: &SourceId,
        job_id: &str,
        track_id: &TrackId,
        completed: bool,
    ) {
        let Some(jobs) = self.jobs.get_mut(source_id) else {
            return;
        };
        let Some(job_index) = jobs.iter().position(|job| job.id == job_id) else {
            return;
        };
        let job = &mut jobs[job_index];
        job.remaining.retain(|candidate| candidate != track_id);
        if completed {
            job.completed.push(track_id.clone());
        }
        if job.remaining.is_empty() {
            jobs.remove(job_index);
        } else {
            job.state = DownloadQueueState::Downloading;
        }
    }

    fn retry_waiting(&mut self) {
        for (source_id, jobs) in &mut self.jobs {
            let attached = self.attached.get(source_id);
            let available = attached.is_some_and(|attached| {
                attached
                    .source
                    .as_ref()
                    .is_some_and(|source| source.strong_count() > 0)
                    && attached.loaded.strong_count() > 0
            });
            if let Some(first) = jobs.first_mut()
                && available
                && first.state == DownloadQueueState::WaitingForConnection
            {
                first.state = DownloadQueueState::Queued;
            }
        }
    }

    async fn force_remove(
        &mut self,
        source_id: &SourceId,
        loaded: &Weak<Library>,
        track_ids: Vec<TrackId>,
        notify: bool,
    ) {
        let remove = track_ids.iter().cloned().collect::<HashSet<_>>();
        for job in self.jobs.entry(source_id.clone()).or_default().iter_mut() {
            job.remaining.retain(|track_id| !remove.contains(track_id));
            job.completed.retain(|track_id| !remove.contains(track_id));
            job.total_tracks = job.remaining.len() + job.completed.len();
        }
        self.jobs
            .entry(source_id.clone())
            .or_default()
            .retain(|job| !job.remaining.is_empty());
        self.reconcile_staging(source_id).await;

        let (removed, failed) = self
            .delete_downloads(source_id, Some(loaded), track_ids)
            .await;
        self.persist_and_publish(source_id).await;
        if notify {
            self.send_removal_notice(removed, failed).await;
        }
    }

    async fn delete_downloads(
        &self,
        source_id: &SourceId,
        loaded: Option<&Weak<Library>>,
        track_ids: impl IntoIterator<Item = TrackId>,
    ) -> (usize, usize) {
        let mut removed = 0usize;
        let mut failed = 0usize;
        let records = load_download_records(&self.root, source_id).unwrap_or_default();
        for track_id in track_ids {
            let paths = records
                .get(&track_id)
                .map(|record| record_download_paths(&self.root, source_id, record))
                .unwrap_or_else(|| download_paths(&self.root, source_id, &track_id));
            match remove_download_files(&paths).await {
                Ok(was_present) => {
                    if let Some(loaded) = loaded.and_then(Weak::upgrade) {
                        let _ = loaded.remove_downloaded_file(&track_id);
                    }
                    removed += usize::from(was_present);
                }
                Err(error) => {
                    failed += 1;
                    warn!(%error, %source_id, %track_id, "could not remove downloaded track");
                }
            }
        }
        (removed, failed)
    }

    async fn send_removal_notice(&self, removed: usize, failed: usize) {
        let message = match (removed, failed) {
            (0, 0) => "This track is not downloaded".to_string(),
            (1, 0) => "Removed 1 download".to_string(),
            (count, 0) => format!("Removed {count} downloads"),
            (_, failed) => format!("Could not remove {failed} downloads"),
        };
        let _ = self.events.send(DownloadEvent::Notice(message)).await;
    }

    async fn remove_rule(
        &mut self,
        source_id: &SourceId,
        loaded: Option<&Weak<Library>>,
        rule: DownloadRule,
        delete_downloads: bool,
    ) {
        let subject = DownloadSubject::Rule(rule);
        self.jobs
            .entry(source_id.clone())
            .or_default()
            .retain(|job| job.subject != subject);
        self.reconcile_staging(source_id).await;
        self.release_owner(source_id, loaded, &subject, None, !delete_downloads)
            .await;
        self.persist_and_publish(source_id).await;
    }

    async fn cancel(
        &mut self,
        source_id: &SourceId,
        job_id: &str,
        active: &mut Vec<ActiveDownload>,
    ) {
        let exists = self
            .jobs
            .get(source_id)
            .is_some_and(|jobs| jobs.iter().any(|job| job.id == job_id));
        if !exists {
            return;
        }
        self.abort_matching(active, true, |download| {
            download.source_id == *source_id && download.job_id == job_id
        })
        .await;
        self.jobs
            .entry(source_id.clone())
            .or_default()
            .retain(|job| job.id != job_id);
        self.reconcile_staging(source_id).await;
        self.persist_and_publish(source_id).await;
    }

    async fn clear_job(
        &mut self,
        source_id: &SourceId,
        job_id: &str,
        active: &mut Vec<ActiveDownload>,
    ) {
        let Some(job) = self
            .jobs
            .get(source_id)
            .and_then(|jobs| jobs.iter().find(|job| job.id == job_id))
            .cloned()
        else {
            return;
        };
        let completed = job
            .completed
            .into_iter()
            .chain(
                self.abort_matching(active, true, |download| {
                    download.source_id == *source_id && download.job_id == job_id
                })
                .await,
            )
            .collect::<HashSet<_>>();
        let subject = job.subject;
        self.jobs
            .entry(source_id.clone())
            .or_default()
            .retain(|job| job.id != job_id);
        self.reconcile_staging(source_id).await;
        let loaded = self
            .attached
            .get(source_id)
            .map(|attached| attached.loaded.clone());
        self.release_owner(
            source_id,
            loaded.as_ref(),
            &subject,
            Some(&completed),
            false,
        )
        .await;
        self.persist_and_publish(source_id).await;
    }

    async fn release_owner(
        &self,
        source_id: &SourceId,
        loaded: Option<&Weak<Library>>,
        subject: &DownloadSubject,
        track_ids: Option<&HashSet<TrackId>>,
        retain: bool,
    ) {
        if track_ids.is_some_and(HashSet::is_empty) {
            return;
        }
        let records = match load_download_records(&self.root, source_id) {
            Ok(records) => records,
            Err(error) => {
                warn!(%error, %source_id, "could not read download ownership");
                return;
            }
        };
        let owner = DownloadOwner::Subject(subject.clone());
        for (track_id, mut record) in records {
            if track_ids.is_some_and(|track_ids| !track_ids.contains(&track_id)) {
                continue;
            }
            if !record.owners.remove(&owner) {
                continue;
            }
            if retain {
                record.owners.insert(DownloadOwner::Retained);
            }
            record
                .owners
                .extend(self.queued_owners_for_track(source_id, &track_id, None));
            let paths = record_download_paths(&self.root, source_id, &record);
            if record.owners.is_empty() {
                if let Err(error) = remove_download_files(&paths).await {
                    warn!(%error, %source_id, %track_id, "could not remove unowned download");
                    continue;
                }
                if let Some(loaded) = loaded.and_then(Weak::upgrade) {
                    let _ = loaded.remove_downloaded_file(&track_id);
                }
            } else if let Err(error) = write_record(&paths, &record).await {
                warn!(%error, %source_id, %track_id, "could not update download ownership");
            }
        }
    }

    async fn move_job(
        &mut self,
        source_id: &SourceId,
        job_id: &str,
        target_job_id: &str,
        after: bool,
    ) {
        let changed = reorder_jobs(
            self.jobs.entry(source_id.clone()).or_default(),
            job_id,
            target_job_id,
            after,
        );
        if changed {
            self.persist_and_publish(source_id).await;
        } else {
            self.publish(source_id).await;
        }
    }

    async fn clear(&mut self, source_id: &SourceId, loaded: Option<&Weak<Library>>, notify: bool) {
        let staging_directory = self
            .attached
            .get(source_id)
            .and_then(|attached| attached.directory.clone());
        self.jobs.remove(source_id);
        let directory = source_directory(&self.root, source_id);
        let result = async {
            cleanup_staging(
                &self.root,
                source_id,
                staging_directory.as_deref(),
                &HashSet::new(),
            )
            .await
            .map_err(|error| error.to_string())?;
            for record in load_download_records(&self.root, source_id)? {
                let paths = record_download_paths(&self.root, source_id, &record.1);
                remove_download_files(&paths).await?;
            }
            match tokio::fs::remove_dir_all(&directory).await {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!("could not remove {}: {error}", directory.display())),
            }
        }
        .await;
        match result {
            Ok(()) => {
                if let Some(loaded) = loaded.and_then(Weak::upgrade) {
                    let _ = loaded.replace_downloaded_files(HashMap::new());
                }
                self.publish(source_id).await;
                if notify {
                    let _ = self
                        .events
                        .send(DownloadEvent::Notice("Removed all downloads".to_string()))
                        .await;
                }
            }
            Err(error) => {
                warn!(%error, %source_id, "could not clear source downloads");
                if notify {
                    let _ = self
                        .events
                        .send(DownloadEvent::Notice(
                            "Could not remove all downloads".to_string(),
                        ))
                        .await;
                }
            }
        }
    }

    async fn persist_and_publish(&self, source_id: &SourceId) {
        if let Err(error) = persist_queue(
            &self.root,
            source_id,
            self.jobs.get(source_id).map(Vec::as_slice).unwrap_or(&[]),
        )
        .await
        {
            warn!(%error, %source_id, "could not save the download queue");
        }
        self.publish(source_id).await;
    }

    async fn publish_all(&self) {
        let source_ids = self.jobs.keys().cloned().collect::<Vec<_>>();
        for source_id in source_ids {
            self.publish(&source_id).await;
        }
    }

    async fn publish(&self, source_id: &SourceId) {
        let downloaded_tracks = self
            .attached
            .get(source_id)
            .and_then(|attached| attached.loaded.upgrade())
            .and_then(|loaded| loaded.downloaded_track_ids().ok())
            .map_or(0, |tracks| tracks.len());
        let jobs = self
            .jobs
            .get(source_id)
            .into_iter()
            .flatten()
            .map(|job| DownloadQueueItem {
                id: job.id.clone(),
                source_id: source_id.clone(),
                subject: job.subject.clone(),
                quality: job.quality,
                completed_tracks: job.completed.len(),
                total_tracks: job.total_tracks,
                state: job.state,
            })
            .collect::<Vec<_>>();
        let _ = self
            .events
            .send(DownloadEvent::Queue {
                source_id: source_id.clone(),
                snapshot: Arc::new(DownloadQueueSnapshot {
                    jobs: jobs.into(),
                    downloaded_tracks,
                    paused: self.paused,
                }),
            })
            .await;
    }
}

async fn download_track(
    source: Arc<Source>,
    track_id: TrackId,
    quality: DownloadQuality,
    paths: DownloadPaths,
    transfers: Arc<TransferClients>,
    cancellation: tokio::sync::oneshot::Receiver<()>,
) -> Result<(), DownloadFailure> {
    match run_transfer(&source, track_id, quality, &paths, &transfers, cancellation).await {
        Ok(NativeSourceResult::Available(())) => Ok(()),
        Ok(NativeSourceResult::Unavailable) => Err(DownloadFailure::NeedsAttention(
            "the selected source does not support downloads".to_string(),
        )),
        Err(error) => Err(download_source_failure(error)),
    }
}

fn download_source_failure(error: SourceError) -> DownloadFailure {
    match error {
        SourceError::NotFound => DownloadFailure::Item(error.to_string()),
        SourceError::Server { status, .. } if status < 500 && status != 429 => {
            DownloadFailure::Item(error.to_string())
        }
        SourceError::Tls(_)
        | SourceError::Network(_)
        | SourceError::Server { .. }
        | SourceError::Cancelled => DownloadFailure::Retry(error.to_string()),
        SourceError::Auth(_)
        | SourceError::InvalidRequest(_)
        | SourceError::InvalidConfig(_)
        | SourceError::Other(_) => DownloadFailure::NeedsAttention(error.to_string()),
    }
}

fn job_id(source_id: &SourceId, subject: &DownloadSubject, sequence: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let input = serde_json::to_vec(&(source_id, subject, now, sequence)).unwrap_or_default();
    hash_id_bytes(&input)
}

fn reorder_jobs(
    jobs: &mut Vec<DownloadJob>,
    job_id: &str,
    target_job_id: &str,
    after: bool,
) -> bool {
    if job_id == target_job_id {
        return false;
    }
    let Some(source_index) = jobs.iter().position(|job| job.id == job_id) else {
        return false;
    };
    let job = jobs.remove(source_index);
    let Some(target_index) = jobs
        .iter()
        .position(|candidate| candidate.id == target_job_id)
    else {
        jobs.insert(source_index, job);
        return false;
    };
    jobs.insert(target_index + usize::from(after), job);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use library::{
        CandidateBatch, CandidateFinish, CandidateHeader, HomeFacts, Libraries, TrackData,
        TrackRelations,
    };
    use proptest::prelude::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;
    use std::sync::mpsc;

    fn test_actor(root: &Path) -> Actor {
        Actor {
            root: Arc::new(root.to_path_buf()),
            events: async_channel::unbounded().0,
            transfers: Arc::new(TransferClients::default()),
            attached: HashMap::new(),
            jobs: HashMap::new(),
            paused: false,
            next_job: 0,
        }
    }

    fn accepted_track(
        root: &Path,
        source_id: SourceId,
        track_id: TrackId,
    ) -> (Arc<Library>, Track) {
        let (loaded, mut tracks) = accepted_tracks(root, source_id, vec![track_id]);
        (loaded, tracks.remove(0))
    }

    fn accepted_tracks(
        root: &Path,
        source_id: SourceId,
        track_ids: Vec<TrackId>,
    ) -> (Arc<Library>, Vec<Track>) {
        let library = Libraries::open(root.join("library.db")).expect("open test Library");
        let tracks = track_ids
            .into_iter()
            .enumerate()
            .map(|(index, track_id)| {
                Track::new(TrackData {
                    id: track_id,
                    album_id: None,
                    title: format!("Offline track {index}"),
                    artist: "Artist".to_string(),
                    album: "Album".to_string(),
                    album_artwork: None,
                    year: 0,
                    release_date: None,
                    date_added: None,
                    last_played: None,
                    play_count: None,
                    user_rating: None,
                    duration_seconds: 180,
                    favorite: false,
                    disc_number: 1,
                    track_number: index as u16 + 1,
                    image_ref: None,
                    local_artwork: None,
                    musicbrainz_recording_id: None,
                    musicbrainz_release_track_id: None,
                    source_path: None,
                    cue: None,
                    source_format: Some("flac".to_string()),
                    comment: None,
                    skip_count: None,
                    bpm: None,
                    relations: TrackRelations::default(),
                })
            })
            .collect::<Vec<_>>();
        let mut candidate = library
            .begin_source_candidate(CandidateHeader {
                source_id,
                input_version: 1,
                input_digest: [4; 32],
            })
            .expect("begin source candidate");
        candidate
            .write(CandidateBatch::Tracks(tracks.clone()))
            .expect("write track");
        let loaded = candidate
            .finish(
                CandidateFinish {
                    freshness: None,
                    home: HomeFacts::RufinDefined,
                    accepted_at: 1,
                },
                None,
            )
            .and_then(library::PreparedSourceCandidate::accept)
            .expect("accept source")
            .library;
        (loaded, tracks)
    }

    fn retained_record(source_id: SourceId, track_id: TrackId) -> DownloadRecord {
        DownloadRecord {
            version: RECORD_VERSION,
            source_id,
            track_id,
            owners: HashSet::from([DownloadOwner::Retained]),
            audio_root: None,
            audio_path: None,
        }
    }

    fn download_job(
        id: &str,
        subject: DownloadSubject,
        remaining: Vec<TrackId>,
        state: DownloadQueueState,
    ) -> DownloadJob {
        DownloadJob {
            id: id.to_string(),
            subject,
            quality: DownloadQuality::Original,
            total_tracks: remaining.len(),
            completed: Vec::new(),
            remaining,
            state,
        }
    }

    fn remote_source(base_url: &str) -> Arc<Source> {
        Arc::new(
            Source::open(
                sources::SourceConfiguration {
                    source_id: SourceId::new("configured:jellyfin"),
                    kind: "jellyfin".to_string(),
                    name: "Server".to_string(),
                    provider_payload: serde_json::json!({
                        "version": 1,
                        "base_url": base_url,
                        "server_id": null,
                        "user_id": "account",
                        "username": "listener",
                        "trust_invalid_cert": false,
                        "use_jellyfin_instant_mix": false,
                    })
                    .to_string(),
                },
                Some("secret-token".to_string()),
                Some("device-one".to_string()),
            )
            .expect("open remote source"),
        )
    }

    fn pending_download(
        root: &Path,
        source_id: SourceId,
        job_id: &str,
        track_id: TrackId,
    ) -> ActiveDownload {
        let paths = download_paths(root, &source_id, &track_id);
        let (cancellation, cancelled) = tokio::sync::oneshot::channel();
        ActiveDownload {
            source_id,
            job_id: job_id.to_string(),
            subject: DownloadSubject::Track(track_id.clone()),
            track_id,
            paths,
            cancellation: Some(cancellation),
            task: tokio::spawn(async move {
                let _ = cancelled.await;
                Err(DownloadFailure::Retry("cancelled".to_string()))
            }),
        }
    }

    proptest! {
        #[test]
        fn reordering_keeps_every_job_once_and_places_the_moved_job_next_to_its_target(
            count in 2usize..32,
            source_seed in 0usize..32,
            target_seed in 0usize..32,
            after in any::<bool>(),
        ) {
            let mut jobs = (0..count)
                .map(|index| {
                    download_job(
                        &format!("job-{index}"),
                        DownloadSubject::Track(TrackId::fake(index)),
                        vec![TrackId::fake(index)],
                        DownloadQueueState::Queued,
                    )
                })
                .collect::<Vec<_>>();
            let source_index = source_seed % count;
            let mut target_index = target_seed % (count - 1);
            if target_index >= source_index {
                target_index += 1;
            }
            let job_id = jobs[source_index].id.clone();
            let target_job_id = jobs[target_index].id.clone();
            let expected = jobs.iter().map(|job| job.id.clone()).collect::<HashSet<_>>();

            prop_assert!(reorder_jobs(&mut jobs, &job_id, &target_job_id, after));

            prop_assert_eq!(
                jobs.iter().map(|job| job.id.clone()).collect::<HashSet<_>>(),
                expected
            );
            prop_assert_eq!(jobs.len(), count);
            let moved = jobs
                .iter()
                .position(|job| job.id == job_id)
                .expect("moved job");
            let target = jobs
                .iter()
                .position(|job| job.id == target_job_id)
                .expect("target job");
            if after {
                prop_assert_eq!(moved, target + 1);
            } else {
                prop_assert_eq!(moved + 1, target);
            }
        }

    }

    #[tokio::test]
    async fn one_playlist_opens_three_response_bodies() {
        let directory = tempfile::tempdir().expect("temporary downloads");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind download server");
        let address = listener.local_addr().expect("download server address");
        let (requests, received) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let mut streams = Vec::new();
            for _ in 0..MAX_ACTIVE_DOWNLOADS {
                let (mut stream, _) = listener.accept().expect("accept download request");
                let mut request = [0; 4096];
                let received = stream.read(&mut request).expect("read download request");
                assert!(received > 0);
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Type: audio/flac\r\nConnection: close\r\n\r\n")
                    .expect("write download headers");
                stream.flush().expect("flush download headers");
                requests.send(()).expect("record download request");
                streams.push(stream);
            }
            released.recv().expect("release download bodies");
            for mut stream in streams {
                let _ = stream.write_all(b"x");
            }
        });
        let source_id = SourceId::fake(1);
        let track_ids = (0..4).map(TrackId::fake).collect::<Vec<_>>();
        let (loaded, _) = accepted_tracks(directory.path(), source_id.clone(), track_ids.clone());
        let source = remote_source(&format!("http://{address}"));
        let mut actor = test_actor(directory.path());
        actor.attached.insert(
            source_id.clone(),
            AttachedSource {
                source: Some(Arc::downgrade(&source)),
                loaded: Arc::downgrade(&loaded),
                directory: None,
            },
        );
        actor.jobs.insert(
            source_id.clone(),
            vec![download_job(
                "playlist",
                DownloadSubject::Playlist(library::PlaylistId::fake(1)),
                track_ids,
                DownloadQueueState::Queued,
            )],
        );
        let mut active = Vec::new();

        actor.fill_slots(&mut active).await;

        assert_eq!(active.len(), MAX_ACTIVE_DOWNLOADS);
        tokio::task::spawn_blocking(move || {
            for _ in 0..MAX_ACTIVE_DOWNLOADS {
                received
                    .recv_timeout(Duration::from_secs(5))
                    .expect("parallel download request");
            }
        })
        .await
        .expect("wait for parallel download requests");
        release.send(()).expect("release download bodies");
        actor.abort_matching(&mut active, false, |_| true).await;
        server.join().expect("download server");
    }

    #[tokio::test]
    async fn attach_removes_downloads_absent_from_the_accepted_source() {
        let directory = tempfile::tempdir().expect("temporary downloads");
        let source_id = SourceId::fake(1);
        let track_id = TrackId::fake(2);
        let paths = download_paths(directory.path(), &source_id, &track_id);
        std::fs::create_dir_all(&paths.directory).expect("download source directory");
        std::fs::write(&paths.audio, b"audio").expect("download audio");
        let record = retained_record(source_id.clone(), track_id);
        std::fs::write(
            &paths.record,
            serde_json::to_vec(&record).expect("encode record"),
        )
        .expect("download record");
        let library =
            Libraries::open(directory.path().join("library.db")).expect("open test Library");
        let loaded = library
            .begin_source_candidate(CandidateHeader {
                source_id,
                input_version: 1,
                input_digest: [1; 32],
            })
            .expect("begin source candidate")
            .finish(
                CandidateFinish {
                    freshness: None,
                    home: HomeFacts::RufinDefined,
                    accepted_at: 1,
                },
                None,
            )
            .and_then(|prepared| prepared.accept())
            .expect("accept empty source")
            .library;

        for stale in attach_downloaded_files(directory.path(), &loaded).expect("attach downloads") {
            remove_download_files(&stale)
                .await
                .expect("remove download");
        }

        assert!(!paths.audio.exists());
        assert!(!paths.record.exists());
    }

    #[tokio::test]
    async fn an_unreadable_initial_queue_keeps_staging() {
        let directory = tempfile::tempdir().expect("temporary downloads");
        let source_id = SourceId::fake(1);
        let track_id = TrackId::fake(2);
        let (loaded, _) = accepted_track(directory.path(), source_id.clone(), track_id.clone());
        let paths = staging_paths(directory.path(), &source_id, &track_id, None);
        let mut checkpoint = paths.audio_part.as_os_str().to_os_string();
        checkpoint.push(".resume");
        let checkpoint = PathBuf::from(checkpoint);
        std::fs::create_dir_all(&paths.directory).expect("download source directory");
        std::fs::write(&paths.audio_part, b"partial").expect("partial download");
        std::fs::write(
            &checkpoint,
            br#"{"representation":"same","validator":"\"v1\"","length":7}"#,
        )
        .expect("resume checkpoint");
        let queue = source_directory(directory.path(), &source_id).join(QUEUE_FILE);
        std::fs::write(&queue, b"not a queue").expect("corrupt queue");
        let mut actor = test_actor(directory.path());

        assert!(
            actor
                .attach(source_id.clone(), None, Arc::downgrade(&loaded), None,)
                .await
                .is_err()
        );

        assert!(
            actor
                .attached
                .get(&source_id)
                .and_then(|attached| attached.loaded.upgrade())
                .is_some()
        );
        assert!(!actor.jobs.contains_key(&source_id));
        assert!(paths.audio_part.is_file());
        assert!(checkpoint.is_file());
        assert_eq!(std::fs::read(queue).expect("saved queue"), b"not a queue");

        assert!(
            actor
                .attach(source_id.clone(), None, Arc::downgrade(&loaded), None)
                .await
                .is_err()
        );
        assert!(!actor.jobs.contains_key(&source_id));
        assert!(paths.audio_part.is_file());
        assert!(checkpoint.is_file());
    }

    #[tokio::test]
    async fn reattaching_keeps_the_live_queue_when_disk_is_empty() {
        let directory = tempfile::tempdir().expect("temporary downloads");
        let source_id = SourceId::fake(1);
        let track_id = TrackId::fake(2);
        let (loaded, _) = accepted_track(directory.path(), source_id.clone(), track_id.clone());
        let mut actor = test_actor(directory.path());
        actor.jobs.insert(
            source_id.clone(),
            vec![download_job(
                "live",
                DownloadSubject::Track(track_id.clone()),
                vec![track_id],
                DownloadQueueState::WaitingForConnection,
            )],
        );

        actor
            .attach(source_id.clone(), None, Arc::downgrade(&loaded), None)
            .await
            .expect("reattach downloads");

        assert_eq!(actor.jobs[&source_id][0].id, "live");
        assert_eq!(
            load_queue(directory.path(), &source_id).expect("saved live queue")[0].id,
            "live"
        );
    }

    #[tokio::test]
    async fn replacing_a_source_does_not_commit_its_old_transfer() {
        let directory = tempfile::tempdir().expect("temporary downloads");
        let source_id = SourceId::new("configured:jellyfin");
        let track_id = TrackId::fake(1);
        let (loaded, _) = accepted_track(directory.path(), source_id.clone(), track_id.clone());
        let old_source = remote_source("http://old.invalid");
        let new_source = remote_source("http://new.invalid");
        let paths = download_paths(directory.path(), &source_id, &track_id);
        std::fs::create_dir_all(&paths.directory).expect("download source directory");
        std::fs::write(&paths.audio_part, b"old source audio").expect("completed transfer");

        let mut actor = test_actor(directory.path());
        actor.attached.insert(
            source_id.clone(),
            AttachedSource {
                source: Some(Arc::downgrade(&old_source)),
                loaded: Arc::downgrade(&loaded),
                directory: None,
            },
        );
        actor.jobs.insert(
            source_id.clone(),
            vec![download_job(
                "active",
                DownloadSubject::Track(track_id.clone()),
                vec![track_id.clone()],
                DownloadQueueState::Downloading,
            )],
        );
        let (cancellation, _cancelled) = tokio::sync::oneshot::channel();
        let mut active = vec![ActiveDownload {
            source_id: source_id.clone(),
            job_id: "active".to_string(),
            track_id: track_id.clone(),
            subject: DownloadSubject::Track(track_id),
            paths: paths.clone(),
            cancellation: Some(cancellation),
            task: tokio::spawn(async { Ok(()) }),
        }];
        let (response, _result) = async_channel::bounded(1);

        actor
            .apply(
                Command::Attach {
                    source_id: source_id.clone(),
                    source: Some(Arc::downgrade(&new_source)),
                    loaded: Arc::downgrade(&loaded),
                    directory: None,
                    response,
                },
                &mut active,
            )
            .await;

        assert!(active.is_empty());
        assert!(!paths.audio.exists());
        assert!(!paths.record.exists());
        assert!(paths.audio_part.exists());
        assert_eq!(actor.jobs[&source_id][0].state, DownloadQueueState::Queued);
        assert!(Arc::ptr_eq(
            &actor.attached[&source_id]
                .source
                .as_ref()
                .and_then(Weak::upgrade)
                .expect("new source remains attached"),
            &new_source,
        ));
    }

    #[test]
    fn attached_downloads_do_not_own_the_library_lifecycle() {
        let directory = tempfile::tempdir().expect("temporary downloads");
        let source_id = SourceId::fake(1);
        let (loaded, _) = accepted_track(directory.path(), source_id.clone(), TrackId::fake(2));
        let loaded_weak = Arc::downgrade(&loaded);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build test runtime");
        let (events, receiver) = async_channel::unbounded();
        let downloads = Downloads::new(
            directory.path().join("downloads"),
            runtime.handle().clone(),
            events,
        );

        runtime
            .block_on(downloads.attach(None, &loaded, None))
            .expect("attach downloaded files");
        let event = runtime.block_on(receiver.recv()).expect("download queue");
        assert!(matches!(
            event,
            DownloadEvent::Queue {
                source_id: event_source,
                ..
            } if event_source == source_id
        ));

        drop(loaded);
        assert!(
            loaded_weak.upgrade().is_none(),
            "Downloads must not retain an inactive Library"
        );
    }

    #[tokio::test]
    async fn pause_preserves_the_queue_and_partial_transfer_until_continue() {
        let directory = tempfile::tempdir().expect("temporary downloads");
        let source_id = SourceId::new("configured:jellyfin");
        let track_id = TrackId::fake(2);
        let queued_track_id = TrackId::fake(3);
        let (loaded, mut tracks) = accepted_tracks(
            directory.path(),
            source_id.clone(),
            vec![track_id.clone(), queued_track_id],
        );
        let queued_track = tracks.remove(1);
        let source = remote_source("http://127.0.0.1:9");
        let paths = download_paths(directory.path(), &source_id, &track_id);
        std::fs::create_dir_all(&paths.directory).expect("download source directory");
        std::fs::write(&paths.audio_part, b"partial").expect("partial download");
        let mut actor = test_actor(directory.path());
        actor.jobs.insert(
            source_id.clone(),
            vec![download_job(
                "active",
                DownloadSubject::Track(track_id.clone()),
                vec![track_id.clone()],
                DownloadQueueState::Downloading,
            )],
        );
        actor.attached.insert(
            source_id.clone(),
            AttachedSource {
                source: Some(Arc::downgrade(&source)),
                loaded: Arc::downgrade(&loaded),
                directory: None,
            },
        );
        let mut active = vec![pending_download(
            directory.path(),
            source_id.clone(),
            "active",
            track_id,
        )];

        actor.apply(Command::SetPaused(true), &mut active).await;

        assert!(actor.paused);
        assert!(active.is_empty());
        assert!(paths.audio_part.exists());
        assert_eq!(actor.jobs[&source_id][0].state, DownloadQueueState::Queued);
        actor.fill_slots(&mut active).await;
        assert!(active.is_empty());
        actor
            .enqueue(
                source_id.clone(),
                DownloadSubject::Playlist(library::PlaylistId::fake(1)),
                DownloadQuality::Original,
                vec![queued_track],
            )
            .await;
        assert_eq!(
            actor.jobs[&source_id]
                .iter()
                .find(|job| {
                    job.subject == DownloadSubject::Playlist(library::PlaylistId::fake(1))
                })
                .expect("connected paused queue")
                .state,
            DownloadQueueState::Queued
        );

        actor.apply(Command::SetPaused(false), &mut active).await;
        assert!(!actor.paused);
    }

    #[tokio::test]
    async fn cancel_interrupts_only_its_transfer_and_keeps_staging_still_in_demand() {
        let directory = tempfile::tempdir().expect("temporary downloads");
        let source_id = SourceId::fake(1);
        let track_id = TrackId::fake(2);
        let other_track_id = TrackId::fake(3);
        let job_id = "active".to_string();
        let other_job_id = "other".to_string();
        let overlap_job_id = "overlap".to_string();
        let paths = download_paths(directory.path(), &source_id, &track_id);
        let other_paths = download_paths(directory.path(), &source_id, &other_track_id);
        std::fs::create_dir_all(&paths.directory).expect("download source directory");
        std::fs::write(&paths.audio_part, b"partial").expect("partial download");
        std::fs::write(
            &paths.checkpoint,
            br#"{"representation":"same","validator":"\"v1\"","length":7}"#,
        )
        .expect("resume checkpoint");
        std::fs::write(&other_paths.audio_part, b"other partial").expect("other partial download");
        let mut actor = test_actor(directory.path());
        actor.jobs.insert(
            source_id.clone(),
            vec![
                download_job(
                    &job_id,
                    DownloadSubject::Track(track_id.clone()),
                    vec![track_id.clone()],
                    DownloadQueueState::Downloading,
                ),
                download_job(
                    &other_job_id,
                    DownloadSubject::Track(other_track_id.clone()),
                    vec![other_track_id.clone()],
                    DownloadQueueState::Downloading,
                ),
                download_job(
                    &overlap_job_id,
                    DownloadSubject::Rule(DownloadRule::Favorites),
                    vec![track_id.clone()],
                    DownloadQueueState::Queued,
                ),
            ],
        );
        let mut active = vec![
            pending_download(directory.path(), source_id.clone(), &job_id, track_id),
            pending_download(
                directory.path(),
                source_id.clone(),
                &other_job_id,
                other_track_id,
            ),
        ];
        actor
            .apply(
                Command::Cancel {
                    source_id: source_id.clone(),
                    job_id,
                },
                &mut active,
            )
            .await;

        assert_eq!(active.len(), 1);
        assert_eq!(active[0].job_id, other_job_id);
        assert_eq!(
            actor
                .jobs
                .get(&source_id)
                .expect("remaining source queue")
                .iter()
                .map(|job| job.id.as_str())
                .collect::<Vec<_>>(),
            ["other", "overlap"]
        );
        assert!(paths.audio_part.exists());
        assert!(paths.checkpoint.exists());
        assert!(other_paths.audio_part.exists());

        actor
            .cancel(&source_id, &overlap_job_id, &mut Vec::new())
            .await;

        assert!(!paths.audio_part.exists());
        assert!(!paths.checkpoint.exists());
        actor.abort_matching(&mut active, false, |_| true).await;
    }

    #[tokio::test]
    async fn cancel_keeps_a_completion_while_clear_removes_it() {
        let directory = tempfile::tempdir().expect("temporary downloads");
        let source_id = SourceId::fake(1);
        let first_track = TrackId::fake(2);
        let second_track = TrackId::fake(3);
        let subject = DownloadSubject::Playlist(library::PlaylistId::fake(1));
        let job_id = "playlist";
        let second_paths = download_paths(directory.path(), &source_id, &second_track);
        std::fs::create_dir_all(&second_paths.directory).expect("download source directory");
        std::fs::write(&second_paths.audio_part, b"second audio").expect("completed transfer");
        let mut actor = test_actor(directory.path());
        actor.jobs.insert(
            source_id.clone(),
            vec![download_job(
                job_id,
                subject.clone(),
                vec![first_track.clone(), second_track.clone()],
                DownloadQueueState::Downloading,
            )],
        );
        let mut remaining_active = vec![pending_download(
            directory.path(),
            source_id.clone(),
            job_id,
            first_track.clone(),
        )];
        let (cancellation, _cancelled) = tokio::sync::oneshot::channel();
        let completed = ActiveDownload {
            source_id: source_id.clone(),
            job_id: job_id.to_string(),
            track_id: second_track.clone(),
            subject: subject.clone(),
            paths: second_paths.clone(),
            cancellation: Some(cancellation),
            task: tokio::spawn(async { Ok(()) }),
        };

        actor
            .finish(completed, Ok(Ok(())), &mut remaining_active)
            .await;

        let job = &actor.jobs.get(&source_id).expect("source queue")[0];
        assert_eq!(job.remaining, [first_track]);
        assert!(second_paths.audio.is_file());
        assert!(second_paths.record.is_file());
        assert_eq!(remaining_active.len(), 1);
        actor
            .apply(
                Command::Cancel {
                    source_id: source_id.clone(),
                    job_id: job_id.to_string(),
                },
                &mut remaining_active,
            )
            .await;

        assert!(remaining_active.is_empty());
        assert!(actor.jobs.get(&source_id).is_none_or(Vec::is_empty));
        assert!(second_paths.audio.exists());
        assert!(second_paths.record.exists());

        let mut clear_job = download_job(
            "clear",
            subject,
            vec![TrackId::fake(4)],
            DownloadQueueState::Queued,
        );
        clear_job.total_tracks = 2;
        clear_job.completed.push(second_track);
        actor.jobs.insert(source_id.clone(), vec![clear_job]);
        actor.clear_job(&source_id, "clear", &mut Vec::new()).await;

        assert!(actor.jobs[&source_id].is_empty());
        assert!(!second_paths.audio.exists());
        assert!(!second_paths.record.exists());
    }

    #[tokio::test]
    async fn reconciling_a_rule_replaces_its_queue_and_releases_stale_owners() {
        let directory = tempfile::tempdir().expect("temporary downloads");
        let source_id = SourceId::fake(1);
        let desired_id = TrackId::fake(2);
        let stale_id = TrackId::fake(3);
        let shared_id = TrackId::fake(4);
        let completed_id = TrackId::fake(5);
        let (loaded, desired_tracks) = accepted_tracks(
            directory.path(),
            source_id.clone(),
            vec![desired_id.clone(), completed_id.clone()],
        );
        for (track_id, owners) in [
            (
                stale_id.clone(),
                HashSet::from([DownloadOwner::Subject(DownloadSubject::Rule(
                    DownloadRule::Favorites,
                ))]),
            ),
            (
                shared_id.clone(),
                HashSet::from([DownloadOwner::Subject(DownloadSubject::Rule(
                    DownloadRule::Favorites,
                ))]),
            ),
            (
                completed_id.clone(),
                HashSet::from([DownloadOwner::Subject(DownloadSubject::Rule(
                    DownloadRule::Favorites,
                ))]),
            ),
        ] {
            let paths = download_paths(directory.path(), &source_id, &track_id);
            std::fs::create_dir_all(&paths.directory).expect("download source directory");
            std::fs::write(&paths.audio, b"audio").expect("download audio");
            write_record(
                &paths,
                &DownloadRecord {
                    version: RECORD_VERSION,
                    source_id: source_id.clone(),
                    track_id,
                    owners,
                    audio_root: None,
                    audio_path: None,
                },
            )
            .await
            .expect("download record");
        }
        let existing_id = "favorites".to_string();
        let mut actor = test_actor(directory.path());
        actor.attached.insert(
            source_id.clone(),
            AttachedSource {
                source: None,
                loaded: Arc::downgrade(&loaded),
                directory: None,
            },
        );
        let mut favorites_job = download_job(
            &existing_id,
            DownloadSubject::Rule(DownloadRule::Favorites),
            vec![stale_id.clone(), shared_id.clone()],
            DownloadQueueState::Queued,
        );
        favorites_job.total_tracks += 1;
        favorites_job.completed.push(completed_id.clone());
        actor.jobs.insert(
            source_id.clone(),
            vec![
                favorites_job,
                download_job(
                    "all-playlists",
                    DownloadSubject::Rule(DownloadRule::AllPlaylists),
                    vec![shared_id.clone()],
                    DownloadQueueState::Queued,
                ),
            ],
        );

        actor
            .reconcile_rule(
                source_id.clone(),
                DownloadRule::Favorites,
                DownloadQuality::Original,
                desired_tracks,
                None,
            )
            .await;

        let stale_paths = download_paths(directory.path(), &source_id, &stale_id);
        assert!(!stale_paths.audio.exists());
        assert!(!stale_paths.record.exists());
        let shared = load_download_records(directory.path(), &source_id)
            .expect("load records")
            .remove(&shared_id)
            .expect("shared record");
        assert!(
            shared
                .owners
                .contains(&DownloadOwner::Subject(DownloadSubject::Rule(
                    DownloadRule::AllPlaylists
                )))
        );
        assert!(
            !shared
                .owners
                .contains(&DownloadOwner::Subject(DownloadSubject::Rule(
                    DownloadRule::Favorites
                )))
        );
        let jobs = actor.jobs.get(&source_id).expect("source queue");
        assert_eq!(jobs.len(), 2);
        let favorites = jobs
            .iter()
            .find(|job| job.id == existing_id)
            .expect("favorites queue");
        assert_eq!(favorites.total_tracks, 2);
        assert_eq!(favorites.completed, vec![completed_id]);
        assert_eq!(favorites.remaining, vec![desired_id]);
    }

    #[tokio::test]
    async fn deleting_a_custom_folder_download_leaves_neighboring_music() {
        let directory = tempfile::tempdir().expect("temporary downloads");
        let custom = tempfile::tempdir().expect("custom download folder");
        let source_id = SourceId::fake(1);
        let (loaded, track) = accepted_track(directory.path(), source_id.clone(), TrackId::fake(2));
        let paths = new_download_paths(
            directory.path(),
            &source_id,
            &track,
            Some(custom.path()),
            DownloadQuality::Original,
        );
        let other_source_paths = new_download_paths(
            directory.path(),
            &SourceId::fake(2),
            &track,
            Some(custom.path()),
            DownloadQuality::Original,
        );
        assert_ne!(paths.audio, other_source_paths.audio);
        assert_eq!(paths.audio.parent(), other_source_paths.audio.parent());
        std::fs::create_dir_all(&paths.directory).expect("create metadata directory");
        std::fs::create_dir_all(paths.audio.parent().expect("album directory"))
            .expect("create album directory");
        std::fs::write(&paths.audio, b"managed audio").expect("download audio");
        let neighboring = paths
            .audio
            .parent()
            .expect("album directory")
            .join("Already Here.flac");
        std::fs::write(&neighboring, b"user audio").expect("neighboring audio");
        let record = DownloadRecord {
            version: RECORD_VERSION,
            source_id: source_id.clone(),
            track_id: track.id.clone(),
            owners: HashSet::from([DownloadOwner::Subject(DownloadSubject::Rule(
                DownloadRule::Favorites,
            ))]),
            audio_root: paths.audio_root.clone(),
            audio_path: Some(paths.audio.clone()),
        };
        write_record(&paths, &record).await.expect("record owner");
        loaded
            .set_downloaded_file(track.id.clone(), paths.audio.clone())
            .expect("attach managed audio");
        let mut actor = test_actor(directory.path());
        let loaded_weak = Arc::downgrade(&loaded);

        actor
            .remove_rule(
                &source_id,
                Some(&loaded_weak),
                DownloadRule::Favorites,
                true,
            )
            .await;

        assert!(!paths.audio.exists());
        assert!(!paths.record.exists());
        assert_eq!(
            std::fs::read(&neighboring).expect("neighboring audio remains"),
            b"user audio"
        );
        assert!(
            !loaded
                .is_downloaded(&track.id)
                .expect("read download status")
        );
    }
}
