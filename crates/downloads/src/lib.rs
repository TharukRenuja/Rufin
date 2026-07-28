//! Durable source downloads exposed through Library's local playback path.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_channel::{Receiver, Sender};
use library::{LoadedLibrary, SourceId, Track, TrackId};
use serde::{Deserialize, Serialize};
use sources::{NativeSourceResult, Source, SourceError, StreamQuality, StreamRequest};
use tracing::warn;

const RECORD_VERSION: u32 = 3;
const QUEUE_VERSION: u32 = 1;
const AUDIO_EXTENSION: &str = "audio";
const RECORD_EXTENSION: &str = "json";
const PART_EXTENSION: &str = "part";
const QUEUE_FILE: &str = "queue.json";
const QUEUE_PART_FILE: &str = "queue.json.part";
const RETRY_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct Downloads {
    root: Arc<PathBuf>,
    commands: Sender<Command>,
}

enum Command {
    Attach {
        source_id: SourceId,
        source: Option<Weak<Source>>,
        loaded: Weak<LoadedLibrary>,
        directory: Option<PathBuf>,
    },
    Download {
        source_id: SourceId,
        source: Option<Weak<Source>>,
        loaded: Weak<LoadedLibrary>,
        directory: Option<PathBuf>,
        subject: DownloadSubject,
        quality: DownloadQuality,
        tracks: Vec<Track>,
    },
    Remove {
        source_id: SourceId,
        loaded: Weak<LoadedLibrary>,
        track_ids: Vec<TrackId>,
        notify: bool,
    },
    RemoveRule {
        source_id: SourceId,
        loaded: Option<Weak<LoadedLibrary>>,
        rule: DownloadRule,
        delete_downloads: bool,
    },
    Cancel {
        source_id: SourceId,
        job_id: String,
    },
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
        loaded: Option<Weak<LoadedLibrary>>,
        notify: bool,
    },
}

#[derive(Clone)]
struct AttachedSource {
    source: Option<Weak<Source>>,
    loaded: Weak<LoadedLibrary>,
    directory: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DownloadRecord {
    version: u32,
    source_id: SourceId,
    track_id: TrackId,
    #[serde(default)]
    owners: HashSet<DownloadOwner>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    audio_root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    audio_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
enum DownloadOwner {
    Subject(DownloadSubject),
    Retained,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DownloadJob {
    id: String,
    source_id: SourceId,
    subject: DownloadSubject,
    quality: DownloadQuality,
    total_tracks: usize,
    completed_tracks: usize,
    remaining: Vec<TrackId>,
    state: DownloadQueueState,
}

#[derive(Debug, Deserialize, Serialize)]
struct QueueFile {
    version: u32,
    source_id: SourceId,
    jobs: Vec<DownloadJob>,
}

struct DownloadPaths {
    directory: PathBuf,
    audio_root: Option<PathBuf>,
    audio: PathBuf,
    audio_part: PathBuf,
    record: PathBuf,
    record_part: PathBuf,
}

enum DownloadFailure {
    Item(String),
    Retry(String),
    NeedsAttention(String),
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
    attached: HashMap<SourceId, AttachedSource>,
    jobs: HashMap<SourceId, Vec<DownloadJob>>,
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
                attached: HashMap::new(),
                jobs: HashMap::new(),
                next_job: 0,
            },
            receiver,
        ));
        downloads
    }

    pub fn attach(
        &self,
        source: Option<Arc<Source>>,
        loaded: &Arc<LoadedLibrary>,
        directory: Option<PathBuf>,
    ) -> Result<(), String> {
        attach_downloaded_files(&self.root, loaded)?;
        self.send(Command::Attach {
            source_id: loaded.source_id().clone(),
            source: source.as_ref().map(Arc::downgrade),
            loaded: Arc::downgrade(loaded),
            directory,
        });
        Ok(())
    }

    pub fn download(
        &self,
        source: Option<Arc<Source>>,
        loaded: Arc<LoadedLibrary>,
        directory: Option<PathBuf>,
        subject: DownloadSubject,
        quality: DownloadQuality,
        tracks: Vec<Track>,
    ) {
        self.send(Command::Download {
            source_id: loaded.source_id().clone(),
            source: source.as_ref().map(Arc::downgrade),
            loaded: Arc::downgrade(&loaded),
            directory,
            subject,
            quality,
            tracks,
        });
    }

    pub fn remove(
        &self,
        source_id: SourceId,
        loaded: Arc<LoadedLibrary>,
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
        loaded: Option<Arc<LoadedLibrary>>,
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

    pub fn clear(&self, source_id: SourceId, loaded: Option<Arc<LoadedLibrary>>, notify: bool) {
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
    loop {
        if let Ok(command) = receiver.try_recv() {
            actor.apply(command).await;
            continue;
        }
        if let Some(source_id) = actor.runnable_source() {
            actor.process(&source_id).await;
            continue;
        }
        tokio::select! {
            command = receiver.recv() => {
                let Ok(command) = command else {
                    break;
                };
                actor.apply(command).await;
            }
            _ = retry.tick() => {
                actor.retry_waiting().await;
            }
        }
    }
}

impl Actor {
    async fn apply(&mut self, command: Command) {
        match command {
            Command::Attach {
                source_id,
                source,
                loaded,
                directory,
            } => self.attach(source_id, source, loaded, directory).await,
            Command::Download {
                source_id,
                source,
                loaded,
                directory,
                subject,
                quality,
                tracks,
            } => {
                self.enqueue(
                    source_id, source, loaded, directory, subject, quality, tracks,
                )
                .await;
            }
            Command::Remove {
                source_id,
                loaded,
                track_ids,
                notify,
            } => {
                self.force_remove(&source_id, &loaded, track_ids, notify)
                    .await;
            }
            Command::RemoveRule {
                source_id,
                loaded,
                rule,
                delete_downloads,
            } => {
                self.remove_rule(&source_id, loaded.as_ref(), rule, delete_downloads)
                    .await;
            }
            Command::Cancel { source_id, job_id } => {
                self.cancel(&source_id, &job_id).await;
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
                if let Some(attached) = self.attached.get_mut(&source_id) {
                    attached.directory = directory;
                }
            }
            Command::Clear {
                source_id,
                loaded,
                notify,
            } => {
                self.clear(&source_id, loaded.as_ref(), notify).await;
            }
        }
    }

    async fn attach(
        &mut self,
        source_id: SourceId,
        source: Option<Weak<Source>>,
        loaded: Weak<LoadedLibrary>,
        directory: Option<PathBuf>,
    ) {
        let Some(live) = loaded.upgrade() else {
            self.attached.remove(&source_id);
            return;
        };
        let source_available = source.as_ref().and_then(Weak::upgrade).is_some();
        let mut jobs = match load_queue(&self.root, &source_id) {
            Ok(jobs) => jobs,
            Err(error) => {
                warn!(%error, %source_id, "could not load the download queue");
                Vec::new()
            }
        };
        jobs.retain_mut(|job| {
            if job.source_id != source_id {
                return false;
            }
            job.remaining
                .retain(|track_id| live.track(track_id).ok().flatten().is_some());
            job.state = if source_available {
                DownloadQueueState::Queued
            } else {
                DownloadQueueState::WaitingForConnection
            };
            !job.remaining.is_empty()
        });
        drop(live);
        self.jobs.insert(source_id.clone(), jobs);
        self.attached.insert(
            source_id.clone(),
            AttachedSource {
                source,
                loaded,
                directory,
            },
        );
        self.persist_and_publish(&source_id).await;
    }

    async fn enqueue(
        &mut self,
        source_id: SourceId,
        source: Option<Weak<Source>>,
        loaded: Weak<LoadedLibrary>,
        directory: Option<PathBuf>,
        subject: DownloadSubject,
        quality: DownloadQuality,
        tracks: Vec<Track>,
    ) {
        let source_available = source.as_ref().and_then(Weak::upgrade).is_some();
        self.attached.insert(
            source_id.clone(),
            AttachedSource {
                source: source.clone(),
                loaded,
                directory,
            },
        );

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
        let mut remaining = Vec::new();
        let mut completed = 0usize;
        for track_id in &track_ids {
            match add_owner_to_existing_download(&self.root, &source_id, track_id, &owner).await {
                Ok(true) => completed += 1,
                Ok(false) => remaining.push(track_id.clone()),
                Err(error) => {
                    warn!(%error, %source_id, %track_id, "could not update download ownership");
                    remaining.push(track_id.clone());
                }
            }
        }

        let mut scheduled_tracks = 0usize;
        if !remaining.is_empty() {
            let jobs = self.jobs.entry(source_id.clone()).or_default();
            if let Some(existing) = jobs
                .iter_mut()
                .find(|job| job.subject == subject && job.quality == quality)
            {
                let mut known = existing.remaining.iter().cloned().collect::<HashSet<_>>();
                let additions = remaining
                    .into_iter()
                    .filter(|track_id| known.insert(track_id.clone()))
                    .collect::<Vec<_>>();
                scheduled_tracks = additions.len();
                existing.total_tracks = existing.total_tracks.saturating_add(additions.len());
                existing.remaining.extend(additions);
                if existing.state == DownloadQueueState::NeedsAttention {
                    existing.state = DownloadQueueState::Queued;
                }
            } else {
                scheduled_tracks = remaining.len();
                self.next_job = self.next_job.wrapping_add(1);
                jobs.push(DownloadJob {
                    id: job_id(&source_id, &subject, self.next_job),
                    source_id: source_id.clone(),
                    subject: subject.clone(),
                    quality,
                    total_tracks: track_ids.len(),
                    completed_tracks: completed,
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
                    kind: if source_available {
                        DownloadFeedbackKind::Started
                    } else {
                        DownloadFeedbackKind::Queued
                    },
                }))
                .await;
        }
    }

    async fn process(&mut self, source_id: &SourceId) {
        loop {
            let Some(attached) = self.attached.get(source_id).cloned() else {
                return;
            };
            let Some(loaded) = attached.loaded.upgrade() else {
                if let Some(job) = self
                    .jobs
                    .get_mut(source_id)
                    .and_then(|jobs| jobs.first_mut())
                {
                    job.state = DownloadQueueState::WaitingForConnection;
                    self.persist_and_publish(source_id).await;
                }
                return;
            };
            let Some(job) = self
                .jobs
                .get_mut(source_id)
                .and_then(|jobs| jobs.first_mut())
            else {
                self.persist_and_publish(source_id).await;
                return;
            };
            if job.state == DownloadQueueState::NeedsAttention {
                return;
            }
            let Some(source) = attached.source.as_ref().and_then(Weak::upgrade) else {
                job.state = DownloadQueueState::WaitingForConnection;
                self.persist_and_publish(source_id).await;
                return;
            };
            let Some(track_id) = job.remaining.first().cloned() else {
                self.jobs
                    .get_mut(source_id)
                    .expect("queue exists")
                    .remove(0);
                self.persist_and_publish(source_id).await;
                continue;
            };
            let track = loaded.track(&track_id).ok().flatten();
            drop(loaded);
            let Some(track) = track else {
                job.remaining.remove(0);
                if job.remaining.is_empty() {
                    self.jobs
                        .get_mut(source_id)
                        .expect("queue exists")
                        .remove(0);
                    self.persist_and_publish(source_id).await;
                } else {
                    self.publish(source_id).await;
                }
                return;
            };
            let subject = job.subject.clone();
            let quality = job.quality;
            let entering_download = job.state != DownloadQueueState::Downloading;
            job.state = DownloadQueueState::Downloading;
            if entering_download {
                self.persist_and_publish(source_id).await;
            } else {
                self.publish(source_id).await;
            }

            let result = download_track(
                &self.root,
                source_id,
                &source,
                &attached.loaded,
                track,
                attached.directory.as_deref(),
                quality,
                DownloadOwner::Subject(subject),
            )
            .await;
            let Some(job) = self
                .jobs
                .get_mut(source_id)
                .and_then(|jobs| jobs.first_mut())
            else {
                return;
            };
            let mut job_finished = false;
            match result {
                Ok(()) => {
                    job.remaining.remove(0);
                    job.completed_tracks = job.completed_tracks.saturating_add(1);
                    if job.remaining.is_empty() {
                        self.jobs
                            .get_mut(source_id)
                            .expect("queue exists")
                            .remove(0);
                        job_finished = true;
                    } else {
                        job.state = DownloadQueueState::Downloading;
                    }
                }
                Err(DownloadFailure::Item(error)) => {
                    warn!(%error, %source_id, %track_id, "could not download track");
                    job.remaining.remove(0);
                    if job.remaining.is_empty() {
                        self.jobs
                            .get_mut(source_id)
                            .expect("queue exists")
                            .remove(0);
                        job_finished = true;
                    } else {
                        job.state = DownloadQueueState::Downloading;
                    }
                }
                Err(DownloadFailure::Retry(error)) => {
                    warn!(%error, %source_id, "download is waiting for the server");
                    job.state = DownloadQueueState::WaitingForConnection;
                    self.persist_and_publish(source_id).await;
                    return;
                }
                Err(DownloadFailure::NeedsAttention(error)) => {
                    warn!(%error, %source_id, "download needs attention");
                    job.state = DownloadQueueState::NeedsAttention;
                    self.persist_and_publish(source_id).await;
                    return;
                }
            }
            if job_finished {
                self.persist_and_publish(source_id).await;
            } else {
                self.publish(source_id).await;
            }
            return;
        }
    }

    async fn retry_waiting(&mut self) {
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

    fn runnable_source(&self) -> Option<SourceId> {
        self.jobs.iter().find_map(|(source_id, jobs)| {
            jobs.first()
                .is_some_and(|job| {
                    matches!(
                        job.state,
                        DownloadQueueState::Queued | DownloadQueueState::Downloading
                    )
                })
                .then(|| source_id.clone())
        })
    }

    async fn force_remove(
        &mut self,
        source_id: &SourceId,
        loaded: &Weak<LoadedLibrary>,
        track_ids: Vec<TrackId>,
        notify: bool,
    ) {
        let remove = track_ids.iter().cloned().collect::<HashSet<_>>();
        for job in self.jobs.entry(source_id.clone()).or_default().iter_mut() {
            job.remaining.retain(|track_id| !remove.contains(track_id));
        }
        self.jobs
            .entry(source_id.clone())
            .or_default()
            .retain(|job| !job.remaining.is_empty());

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
                    if let Some(loaded) = loaded.upgrade() {
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
        self.persist_and_publish(source_id).await;
        if notify {
            let message = match (removed, failed) {
                (0, 0) => "This track is not downloaded".to_string(),
                (1, 0) => "Removed 1 download".to_string(),
                (count, 0) => format!("Removed {count} downloads"),
                (_, failed) => format!("Could not remove {failed} downloads"),
            };
            let _ = self.events.send(DownloadEvent::Notice(message)).await;
        }
    }

    async fn remove_rule(
        &mut self,
        source_id: &SourceId,
        loaded: Option<&Weak<LoadedLibrary>>,
        rule: DownloadRule,
        delete_downloads: bool,
    ) {
        let subject = DownloadSubject::Rule(rule);
        self.jobs
            .entry(source_id.clone())
            .or_default()
            .retain(|job| job.subject != subject);

        let records = match load_download_records(&self.root, source_id) {
            Ok(records) => records,
            Err(error) => {
                warn!(%error, %source_id, "could not read rule downloads");
                return;
            }
        };
        for (track_id, mut record) in records {
            if !record
                .owners
                .remove(&DownloadOwner::Subject(subject.clone()))
            {
                continue;
            }
            if !delete_downloads {
                record.owners.insert(DownloadOwner::Retained);
            }
            let paths = record_download_paths(&self.root, source_id, &record);
            if record.owners.is_empty() {
                if let Err(error) = remove_download_files(&paths).await {
                    warn!(%error, %source_id, %track_id, "could not remove rule download");
                    continue;
                }
                if let Some(loaded) = loaded.and_then(Weak::upgrade) {
                    let _ = loaded.remove_downloaded_file(&track_id);
                }
            } else if let Err(error) = write_record(&paths, &record).await {
                warn!(%error, %source_id, %track_id, "could not update rule download");
            }
        }
        self.persist_and_publish(source_id).await;
    }

    async fn cancel(&mut self, source_id: &SourceId, job_id: &str) {
        self.jobs
            .entry(source_id.clone())
            .or_default()
            .retain(|job| job.id != job_id);
        self.persist_and_publish(source_id).await;
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

    async fn clear(
        &mut self,
        source_id: &SourceId,
        loaded: Option<&Weak<LoadedLibrary>>,
        notify: bool,
    ) {
        self.jobs.remove(source_id);
        let directory = source_directory(&self.root, source_id);
        let result = async {
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
                source_id: job.source_id.clone(),
                subject: job.subject.clone(),
                quality: job.quality,
                completed_tracks: job.completed_tracks,
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
                }),
            })
            .await;
    }
}

async fn download_track(
    root: &Path,
    source_id: &SourceId,
    source: &Source,
    loaded: &Weak<LoadedLibrary>,
    track: Track,
    directory: Option<&Path>,
    quality: DownloadQuality,
    owner: DownloadOwner,
) -> Result<(), DownloadFailure> {
    let track_id = track.id.clone();
    if add_owner_to_existing_download(root, source_id, &track_id, &owner)
        .await
        .map_err(DownloadFailure::NeedsAttention)?
    {
        return Ok(());
    }
    let paths = new_download_paths(root, source_id, &track, directory, quality);
    tokio::fs::create_dir_all(&paths.directory)
        .await
        .map_err(|error| {
            DownloadFailure::NeedsAttention(format!(
                "could not create the downloads directory: {error}"
            ))
        })?;
    if let Some(parent) = paths.audio.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|error| {
            DownloadFailure::NeedsAttention(format!(
                "could not create the download album directory: {error}"
            ))
        })?;
    }
    remove_file_if_present(&paths.audio_part)
        .await
        .map_err(DownloadFailure::NeedsAttention)?;
    remove_file_if_present(&paths.record_part)
        .await
        .map_err(DownloadFailure::NeedsAttention)?;
    let request = StreamRequest::new(track_id.clone(), source_quality(quality));
    match source.download(&request, &paths.audio_part).await {
        Ok(NativeSourceResult::Available(())) => {}
        Ok(NativeSourceResult::Unavailable) => {
            return Err(DownloadFailure::NeedsAttention(
                "the selected source does not support downloads".to_string(),
            ));
        }
        Err(error) => {
            let _ = tokio::fs::remove_file(&paths.audio_part).await;
            return Err(download_source_failure(error));
        }
    }
    remove_file_if_present(&paths.audio)
        .await
        .map_err(DownloadFailure::NeedsAttention)?;
    tokio::fs::rename(&paths.audio_part, &paths.audio)
        .await
        .map_err(|error| {
            DownloadFailure::NeedsAttention(format!("could not save the downloaded track: {error}"))
        })?;
    let record = DownloadRecord {
        version: RECORD_VERSION,
        source_id: source_id.clone(),
        track_id: track_id.clone(),
        owners: HashSet::from([owner]),
        audio_root: paths.audio_root.clone(),
        audio_path: Some(paths.audio.clone()),
    };
    if let Err(error) = write_record(&paths, &record).await {
        let _ = tokio::fs::remove_file(&paths.audio).await;
        return Err(DownloadFailure::NeedsAttention(error));
    }
    if let Some(loaded) = loaded.upgrade()
        && let Err(error) = loaded.set_downloaded_file(track_id, paths.audio.clone())
    {
        let _ = tokio::fs::remove_file(&paths.record).await;
        let _ = tokio::fs::remove_file(&paths.audio).await;
        return Err(DownloadFailure::NeedsAttention(error.to_string()));
    }
    Ok(())
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

async fn add_owner_to_existing_download(
    root: &Path,
    source_id: &SourceId,
    track_id: &TrackId,
    owner: &DownloadOwner,
) -> Result<bool, String> {
    let metadata_paths = download_paths(root, source_id, track_id);
    if !metadata_paths.record.is_file() {
        return Ok(false);
    }
    let bytes = tokio::fs::read(&metadata_paths.record)
        .await
        .map_err(|error| format!("could not read the download record: {error}"))?;
    let mut record = serde_json::from_slice::<DownloadRecord>(&bytes)
        .map_err(|error| format!("could not decode the download record: {error}"))?;
    if record.source_id != *source_id || record.track_id != *track_id {
        return Ok(false);
    }
    let paths = record_download_paths(root, source_id, &record);
    if !paths.audio.is_file() {
        return Ok(false);
    }
    if record.owners.is_empty() {
        record.owners.insert(DownloadOwner::Retained);
    }
    if record.owners.insert(owner.clone()) {
        write_record(&paths, &record).await?;
    }
    Ok(true)
}

async fn write_record(paths: &DownloadPaths, record: &DownloadRecord) -> Result<(), String> {
    let encoded = serde_json::to_vec(record)
        .map_err(|error| format!("could not encode the download record: {error}"))?;
    tokio::fs::write(&paths.record_part, encoded)
        .await
        .map_err(|error| format!("could not save the download record: {error}"))?;
    remove_file_if_present(&paths.record).await?;
    tokio::fs::rename(&paths.record_part, &paths.record)
        .await
        .map_err(|error| format!("could not finish the download record: {error}"))
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
        remove_file_if_present(&path).await?;
        remove_file_if_present(&part).await?;
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
    remove_file_if_present(&path).await?;
    tokio::fs::rename(&part, &path)
        .await
        .map_err(|error| format!("could not finish the download queue: {error}"))
}

fn load_queue(root: &Path, source_id: &SourceId) -> Result<Vec<DownloadJob>, String> {
    let path = source_directory(root, source_id).join(QUEUE_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("could not read {}: {error}", path.display())),
    };
    let queue = serde_json::from_slice::<QueueFile>(&bytes)
        .map_err(|error| format!("could not decode {}: {error}", path.display()))?;
    if queue.version != QUEUE_VERSION || queue.source_id != *source_id {
        return Err("the saved download queue does not match this source".to_string());
    }
    Ok(queue.jobs)
}

fn load_download_records(
    root: &Path,
    source_id: &SourceId,
) -> Result<HashMap<TrackId, DownloadRecord>, String> {
    let directory = source_directory(root, source_id);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => {
            return Err(format!(
                "could not read downloads at {}: {error}",
                directory.display()
            ));
        }
    };
    let mut records = HashMap::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("could not read a download entry: {error}"))?;
        let path = entry.path();
        if path.file_name().and_then(|name| name.to_str()) == Some(QUEUE_FILE) {
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some(RECORD_EXTENSION) {
            continue;
        }
        let Ok(mut record) = std::fs::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<DownloadRecord>(&bytes).map_err(|error| error.to_string())
            })
        else {
            continue;
        };
        if record.version > RECORD_VERSION || record.source_id != *source_id {
            continue;
        }
        if record.owners.is_empty() {
            record.owners.insert(DownloadOwner::Retained);
        }
        records.insert(record.track_id.clone(), record);
    }
    Ok(records)
}

fn load_downloaded_files(
    root: &Path,
    source_id: &SourceId,
) -> Result<HashMap<TrackId, PathBuf>, String> {
    let directory = source_directory(root, source_id);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => {
            return Err(format!(
                "could not read downloads at {}: {error}",
                directory.display()
            ));
        }
    };
    let mut files = HashMap::new();
    let mut referenced_audio = HashSet::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("could not read a download entry: {error}"))?;
        let path = entry.path();
        if path.file_name().and_then(|name| name.to_str()) == Some(QUEUE_PART_FILE) {
            let _ = std::fs::remove_file(path);
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) == Some(PART_EXTENSION) {
            let _ = std::fs::remove_file(path);
            continue;
        }
        if path.file_name().and_then(|name| name.to_str()) == Some(QUEUE_FILE)
            || path.extension().and_then(|value| value.to_str()) != Some(RECORD_EXTENSION)
        {
            continue;
        }
        let record = match std::fs::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<DownloadRecord>(&bytes).map_err(|error| error.to_string())
            }) {
            Ok(record) if record.version <= RECORD_VERSION && record.source_id == *source_id => {
                record
            }
            Ok(_) | Err(_) => {
                warn!(path = %path.display(), "ignored an invalid download record");
                let _ = std::fs::remove_file(path);
                continue;
            }
        };
        let expected = record_download_paths(root, source_id, &record);
        if path != expected.record {
            warn!(path = %path.display(), "ignored a misplaced download record");
            let _ = std::fs::remove_file(path);
            continue;
        }
        if expected.audio.is_file() {
            if expected.audio_root.is_none() {
                referenced_audio.insert(
                    expected
                        .audio
                        .file_name()
                        .expect("download audio path has a file name")
                        .to_os_string(),
                );
            }
            let _ = std::fs::remove_file(&expected.audio_part);
            files.insert(record.track_id, expected.audio);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
    if let Ok(entries) = std::fs::read_dir(&directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) == Some(AUDIO_EXTENSION)
                && path
                    .file_name()
                    .is_some_and(|name| !referenced_audio.contains(name))
            {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    Ok(files)
}

fn attach_downloaded_files(root: &Path, loaded: &Arc<LoadedLibrary>) -> Result<(), String> {
    let source_id = loaded.source_id();
    let files = load_downloaded_files(root, source_id)?;
    let removed = loaded
        .replace_downloaded_files(files)
        .map_err(|error| error.to_string())?;
    let records = load_download_records(root, source_id)?;
    for track_id in removed {
        let paths = records
            .get(&track_id)
            .map(|record| record_download_paths(root, source_id, record))
            .unwrap_or_else(|| download_paths(root, source_id, &track_id));
        remove_download_files_blocking(&paths)?;
    }
    Ok(())
}

async fn remove_download_files(paths: &DownloadPaths) -> Result<bool, String> {
    let mut present = false;
    for path in [
        &paths.audio,
        &paths.audio_part,
        &paths.record,
        &paths.record_part,
    ] {
        match tokio::fs::remove_file(path).await {
            Ok(()) => present = true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("could not remove {}: {error}", path.display())),
        }
    }
    remove_empty_audio_directories(paths).await;
    Ok(present)
}

fn remove_download_files_blocking(paths: &DownloadPaths) -> Result<(), String> {
    for path in [
        &paths.audio,
        &paths.audio_part,
        &paths.record,
        &paths.record_part,
    ] {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("could not remove {}: {error}", path.display())),
        }
    }
    remove_empty_audio_directories_blocking(paths);
    Ok(())
}

async fn remove_empty_audio_directories(paths: &DownloadPaths) {
    let Some(root) = paths.audio_root.as_ref() else {
        return;
    };
    let Some(album) = paths.audio.parent() else {
        return;
    };
    let Some(artist) = album.parent() else {
        return;
    };
    for directory in [album, artist] {
        if directory == root {
            break;
        }
        match tokio::fs::remove_dir(directory).await {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) =>
            {
                break;
            }
            Err(_) => break,
        }
    }
}

fn remove_empty_audio_directories_blocking(paths: &DownloadPaths) {
    let Some(root) = paths.audio_root.as_ref() else {
        return;
    };
    let Some(album) = paths.audio.parent() else {
        return;
    };
    let Some(artist) = album.parent() else {
        return;
    };
    for directory in [album, artist] {
        if directory == root {
            break;
        }
        match std::fs::remove_dir(directory) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) =>
            {
                break;
            }
            Err(_) => break,
        }
    }
}

async fn remove_file_if_present(path: &Path) -> Result<(), String> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("could not replace {}: {error}", path.display())),
    }
}

fn source_directory(root: &Path, source_id: &SourceId) -> PathBuf {
    root.join(hash_id(source_id.as_str()))
}

fn download_paths(root: &Path, source_id: &SourceId, track_id: &TrackId) -> DownloadPaths {
    let directory = source_directory(root, source_id);
    let stem = hash_id(track_id.as_str());
    DownloadPaths {
        audio_root: None,
        audio: directory.join(format!("{stem}.{AUDIO_EXTENSION}")),
        audio_part: directory.join(format!("{stem}.{AUDIO_EXTENSION}.{PART_EXTENSION}")),
        record: directory.join(format!("{stem}.{RECORD_EXTENSION}")),
        record_part: directory.join(format!("{stem}.{RECORD_EXTENSION}.{PART_EXTENSION}")),
        directory,
    }
}

fn new_download_paths(
    root: &Path,
    source_id: &SourceId,
    track: &Track,
    directory: Option<&Path>,
    quality: DownloadQuality,
) -> DownloadPaths {
    let mut paths = download_paths(root, source_id, &track.id);
    let audio_root = directory
        .map(Path::to_path_buf)
        .unwrap_or_else(|| source_directory(root, source_id));
    let artist = safe_path_component(&track.artist, "Unknown Artist");
    let album = safe_path_component(&track.album, "Unknown Album");
    let title = safe_path_component(&track.title, "Untitled");
    let id = hash_id(track.id.as_str());
    let short_id = id.chars().take(12).collect::<String>();
    let extension = download_extension(track, quality);
    let file_name = format!(
        "{:02}-{:02} {title} [{}].{extension}",
        track.disc_number, track.track_number, short_id
    );
    let audio = audio_root.join(artist).join(album).join(file_name);
    let audio_part = part_path(&audio);
    paths.audio_root = Some(audio_root);
    paths.audio = audio;
    paths.audio_part = audio_part;
    paths
}

fn record_download_paths(
    root: &Path,
    source_id: &SourceId,
    record: &DownloadRecord,
) -> DownloadPaths {
    let mut paths = download_paths(root, source_id, &record.track_id);
    let Some(audio_root) = record.audio_root.as_ref() else {
        return paths;
    };
    let Some(audio) = record.audio_path.as_ref() else {
        return paths;
    };
    if !valid_managed_audio_path(audio_root, audio) {
        return paths;
    }
    paths.audio_root = Some(audio_root.clone());
    paths.audio = audio.clone();
    paths.audio_part = part_path(audio);
    paths
}

fn part_path(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(format!(".{PART_EXTENSION}"));
    value.into()
}

fn valid_managed_audio_path(root: &Path, audio: &Path) -> bool {
    let Ok(relative) = audio.strip_prefix(root) else {
        return false;
    };
    let components = relative.components().collect::<Vec<_>>();
    components.len() == 3
        && components
            .iter()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn safe_path_component(value: &str, fallback: &str) -> String {
    let sanitized = value
        .trim()
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
            {
                '_'
            } else {
                character
            }
        })
        .take(80)
        .collect::<String>();
    let sanitized = sanitized.trim_matches([' ', '.']);
    if sanitized.is_empty() {
        fallback.to_string()
    } else {
        sanitized.to_string()
    }
}

fn download_extension(track: &Track, quality: DownloadQuality) -> String {
    if matches!(quality, DownloadQuality::MaxBitrateKbps(_)) {
        return "mp3".to_string();
    }
    let extension = track
        .source_format
        .as_deref()
        .unwrap_or(AUDIO_EXTENSION)
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(12)
        .collect::<String>()
        .to_ascii_lowercase();
    if extension.is_empty() {
        AUDIO_EXTENSION.to_string()
    } else {
        extension
    }
}

fn job_id(source_id: &SourceId, subject: &DownloadSubject, sequence: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut input = serde_json::to_vec(subject).unwrap_or_default();
    input.extend_from_slice(source_id.as_str().as_bytes());
    input.extend_from_slice(&now.to_le_bytes());
    input.extend_from_slice(&sequence.to_le_bytes());
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

fn hash_id(value: &str) -> String {
    hash_id_bytes(value.as_bytes())
}

fn hash_id_bytes(value: &[u8]) -> String {
    blake3::hash(value).to_hex().to_string()
}

fn source_quality(quality: DownloadQuality) -> StreamQuality {
    match quality {
        DownloadQuality::Original => StreamQuality::Original,
        DownloadQuality::MaxBitrateKbps(value) => StreamQuality::MaxBitrateKbps(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use library::{
        CandidateBatch, CandidateFinish, CandidateHeader, HomeFacts, Library, TrackData,
        TrackRelations,
    };

    fn test_actor(root: &Path) -> Actor {
        test_actor_with_events(root).0
    }

    fn test_actor_with_events(root: &Path) -> (Actor, Receiver<DownloadEvent>) {
        let (events, receiver) = async_channel::unbounded();
        (
            Actor {
                root: Arc::new(root.to_path_buf()),
                events,
                attached: HashMap::new(),
                jobs: HashMap::new(),
                next_job: 0,
            },
            receiver,
        )
    }

    fn accepted_track(
        root: &Path,
        source_id: SourceId,
        track_id: TrackId,
    ) -> (Arc<LoadedLibrary>, Track) {
        let library = Library::open(root.join("library.db")).expect("open test Library");
        let track = Track::new(TrackData {
            id: track_id,
            album_id: None,
            title: "Offline track".to_string(),
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
            track_number: 1,
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
        });
        let mut candidate = library
            .begin_source_candidate(CandidateHeader {
                source_id,
                input_version: 1,
                input_digest: [4; 32],
            })
            .expect("begin source candidate");
        candidate
            .write(CandidateBatch::Tracks(vec![track.clone()]))
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
            .loaded;
        (loaded, track)
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

    fn download_job(source_id: &SourceId, id: &str, state: DownloadQueueState) -> DownloadJob {
        DownloadJob {
            id: id.to_string(),
            source_id: source_id.clone(),
            subject: DownloadSubject::Track(TrackId::fake(id.len())),
            quality: DownloadQuality::Original,
            total_tracks: 1,
            completed_tracks: 0,
            remaining: vec![TrackId::fake(id.len())],
            state,
        }
    }

    #[test]
    fn load_accepts_complete_records_and_ignores_the_queue_file() {
        let directory = tempfile::tempdir().expect("temporary downloads");
        let source_id = SourceId::fake(1);
        let track_id = TrackId::fake(2);
        let paths = download_paths(directory.path(), &source_id, &track_id);
        std::fs::create_dir_all(&paths.directory).expect("download source directory");
        std::fs::write(&paths.audio, b"audio").expect("download audio");
        let record = retained_record(source_id.clone(), track_id.clone());
        std::fs::write(
            &paths.record,
            serde_json::to_vec(&record).expect("encode record"),
        )
        .expect("download record");
        std::fs::write(paths.directory.join(QUEUE_FILE), b"{}").expect("queue file");
        std::fs::write(&paths.audio_part, b"partial").expect("partial audio");

        let files = load_downloaded_files(directory.path(), &source_id).expect("load downloads");

        assert_eq!(files.get(&track_id), Some(&paths.audio));
        assert!(!paths.audio_part.exists());
        assert!(paths.directory.join(QUEUE_FILE).exists());
    }

    #[test]
    fn attach_removes_downloads_absent_from_the_accepted_source() {
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
            Library::open(directory.path().join("library.db")).expect("open test Library");
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
            .loaded;

        attach_downloaded_files(directory.path(), &loaded).expect("attach downloads");

        assert!(!paths.audio.exists());
        assert!(!paths.record.exists());
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

        downloads
            .attach(None, &loaded, None)
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
            "Downloads must not retain an inactive LoadedLibrary"
        );
    }

    #[test]
    fn network_failures_wait_but_authentication_needs_attention() {
        assert!(matches!(
            download_source_failure(SourceError::Network("offline".to_string())),
            DownloadFailure::Retry(_)
        ));
        assert!(matches!(
            download_source_failure(SourceError::Server {
                status: 503,
                message: "unavailable".to_string(),
            }),
            DownloadFailure::Retry(_)
        ));
        assert!(matches!(
            download_source_failure(SourceError::Auth("expired".to_string())),
            DownloadFailure::NeedsAttention(_)
        ));
        assert!(matches!(
            download_source_failure(SourceError::NotFound),
            DownloadFailure::Item(_)
        ));
    }

    #[tokio::test]
    async fn an_offline_request_is_persisted_waiting_for_connection() {
        let directory = tempfile::tempdir().expect("temporary downloads");
        let source_id = SourceId::fake(1);
        let (loaded, track) = accepted_track(directory.path(), source_id.clone(), TrackId::fake(2));
        let (mut actor, events) = test_actor_with_events(directory.path());

        actor
            .enqueue(
                source_id.clone(),
                None,
                Arc::downgrade(&loaded),
                None,
                DownloadSubject::Track(track.id.clone()),
                DownloadQuality::Original,
                vec![track],
            )
            .await;

        let jobs = load_queue(directory.path(), &source_id).expect("load saved queue");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].state, DownloadQueueState::WaitingForConnection);
        assert_eq!(jobs[0].quality, DownloadQuality::Original);
        assert!(matches!(
            events.recv().await.expect("queue snapshot"),
            DownloadEvent::Queue { .. }
        ));
        assert!(matches!(
            events.recv().await.expect("queued feedback"),
            DownloadEvent::Feedback(DownloadFeedback {
                kind: DownloadFeedbackKind::Queued,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn queue_reordering_persists_including_the_active_download() {
        let directory = tempfile::tempdir().expect("temporary downloads");
        let source_id = SourceId::fake(1);
        let mut actor = test_actor(directory.path());
        actor.jobs.insert(
            source_id.clone(),
            vec![
                download_job(&source_id, "active", DownloadQueueState::Downloading),
                download_job(&source_id, "second", DownloadQueueState::Queued),
                download_job(&source_id, "third", DownloadQueueState::NeedsAttention),
            ],
        );

        actor.move_job(&source_id, "active", "third", true).await;

        let jobs = actor.jobs.get(&source_id).expect("source queue");
        assert_eq!(
            jobs.iter().map(|job| job.id.as_str()).collect::<Vec<_>>(),
            ["second", "third", "active"]
        );
        let saved = load_queue(directory.path(), &source_id).expect("saved queue");
        assert_eq!(
            saved.iter().map(|job| job.id.as_str()).collect::<Vec<_>>(),
            ["second", "third", "active"]
        );
    }

    #[tokio::test]
    async fn removing_a_rule_can_keep_its_downloads_explicitly() {
        let directory = tempfile::tempdir().expect("temporary downloads");
        let source_id = SourceId::fake(1);
        let track_id = TrackId::fake(2);
        let paths = download_paths(directory.path(), &source_id, &track_id);
        std::fs::create_dir_all(&paths.directory).expect("download source directory");
        std::fs::write(&paths.audio, b"audio").expect("download audio");
        let record = DownloadRecord {
            version: RECORD_VERSION,
            source_id: source_id.clone(),
            track_id: track_id.clone(),
            owners: HashSet::from([DownloadOwner::Subject(DownloadSubject::Rule(
                DownloadRule::Favorites,
            ))]),
            audio_root: None,
            audio_path: None,
        };
        write_record(&paths, &record).await.expect("record owner");
        let mut actor = test_actor(directory.path());

        actor
            .remove_rule(&source_id, None, DownloadRule::Favorites, false)
            .await;

        let restored = load_download_records(directory.path(), &source_id)
            .expect("load records")
            .remove(&track_id)
            .expect("download record");
        assert!(paths.audio.exists());
        assert_eq!(restored.owners, HashSet::from([DownloadOwner::Retained]));
    }

    #[tokio::test]
    async fn deleting_one_rule_keeps_an_overlapping_download() {
        let directory = tempfile::tempdir().expect("temporary downloads");
        let source_id = SourceId::fake(1);
        let track_id = TrackId::fake(2);
        let paths = download_paths(directory.path(), &source_id, &track_id);
        std::fs::create_dir_all(&paths.directory).expect("download source directory");
        std::fs::write(&paths.audio, b"audio").expect("download audio");
        let record = DownloadRecord {
            version: RECORD_VERSION,
            source_id: source_id.clone(),
            track_id: track_id.clone(),
            owners: HashSet::from([
                DownloadOwner::Subject(DownloadSubject::Rule(DownloadRule::Favorites)),
                DownloadOwner::Subject(DownloadSubject::Rule(DownloadRule::AllPlaylists)),
            ]),
            audio_root: None,
            audio_path: None,
        };
        write_record(&paths, &record).await.expect("record owners");
        let mut actor = test_actor(directory.path());

        actor
            .remove_rule(&source_id, None, DownloadRule::Favorites, true)
            .await;

        let restored = load_download_records(directory.path(), &source_id)
            .expect("load records")
            .remove(&track_id)
            .expect("download record");
        assert!(paths.audio.exists());
        assert_eq!(restored.owners.len(), 1);
        assert!(
            restored
                .owners
                .contains(&DownloadOwner::Subject(DownloadSubject::Rule(
                    DownloadRule::AllPlaylists
                )))
        );
    }

    #[tokio::test]
    async fn deleting_the_only_rule_removes_its_download() {
        let directory = tempfile::tempdir().expect("temporary downloads");
        let source_id = SourceId::fake(1);
        let track_id = TrackId::fake(2);
        let paths = download_paths(directory.path(), &source_id, &track_id);
        std::fs::create_dir_all(&paths.directory).expect("download source directory");
        std::fs::write(&paths.audio, b"audio").expect("download audio");
        let record = DownloadRecord {
            version: RECORD_VERSION,
            source_id: source_id.clone(),
            track_id,
            owners: HashSet::from([DownloadOwner::Subject(DownloadSubject::Rule(
                DownloadRule::Favorites,
            ))]),
            audio_root: None,
            audio_path: None,
        };
        write_record(&paths, &record).await.expect("record owner");
        let mut actor = test_actor(directory.path());

        actor
            .remove_rule(&source_id, None, DownloadRule::Favorites, true)
            .await;

        assert!(!paths.audio.exists());
        assert!(!paths.record.exists());
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

    #[test]
    fn custom_paths_use_artist_album_and_quality_extension() {
        let directory = tempfile::tempdir().expect("temporary downloads");
        let custom = tempfile::tempdir().expect("custom download folder");
        let source_id = SourceId::fake(1);
        let (_, track) = accepted_track(directory.path(), source_id.clone(), TrackId::fake(2));

        let original = new_download_paths(
            directory.path(),
            &source_id,
            &track,
            Some(custom.path()),
            DownloadQuality::Original,
        );
        let transcoded = new_download_paths(
            directory.path(),
            &source_id,
            &track,
            Some(custom.path()),
            DownloadQuality::MaxBitrateKbps(192),
        );

        assert_eq!(
            original
                .audio
                .strip_prefix(custom.path())
                .expect("custom relative path")
                .components()
                .count(),
            3
        );
        assert_eq!(
            original.audio.extension().and_then(|value| value.to_str()),
            Some("flac")
        );
        assert_eq!(
            transcoded
                .audio
                .extension()
                .and_then(|value| value.to_str()),
            Some("mp3")
        );
        assert!(original.record.starts_with(directory.path()));
        assert!(!original.record.starts_with(custom.path()));
    }
}
