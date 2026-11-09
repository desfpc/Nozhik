//! Project text search over every indexed path, including files the language
//! index does not parse. Results stay in memory until a path changes or the
//! process asks the cache to clear.
use super::Reply;
use super::fuzzy::FileMatch;
use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

/// Largest file whose bytes are read for one query. Bigger files are skipped.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Matches kept from a single file.
const MAX_HITS_PER_FILE: usize = 20;
/// Matches returned for one query. Later matches stay out of the popup.
const MAX_HITS: usize = 1000;
/// Preview length in Unicode scalars.
const PREVIEW_CHARS: usize = 160;

/// One literal match inside a project file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextHit {
    /// Absolute native path.
    pub path: PathBuf,
    /// Root-relative path using forward slashes.
    pub relative: String,
    /// Zero-based line of the match.
    pub line: u32,
    /// Trimmed line excerpt.
    pub preview: String,
    /// UTF-8 start byte.
    pub byte: usize,
    /// UTF-8 end byte, exclusive.
    pub end: usize,
    /// True when the extension belongs to a supported language index.
    pub indexed: bool,
}

#[derive(Default)]
struct TextCache {
    /// Bumped on forget/clear so an in-flight read cannot repopulate stale bytes.
    epoch: u64,
    /// Query to per-file hits. An empty vector means the file was scanned.
    entries: HashMap<String, HashMap<PathBuf, Vec<TextHit>>>,
}

impl TextCache {
    fn forget(&mut self, path: &Path) {
        self.epoch = self.epoch.wrapping_add(1);
        let canonical = path.canonicalize().ok();
        for files in self.entries.values_mut() {
            files.retain(|candidate, _| {
                !covers(path, candidate) && !covers_canonical(canonical.as_deref(), candidate)
            });
        }
    }

    fn clear(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.entries.clear();
    }

    fn get(&self, query: &str, path: &Path) -> Option<Vec<TextHit>> {
        self.entries.get(query)?.get(path).cloned()
    }

    fn insert(&mut self, query: &str, path: PathBuf, hits: Vec<TextHit>) {
        self.entries
            .entry(query.to_owned())
            .or_default()
            .insert(path, hits);
    }
}

pub struct SearchJob {
    pub token: u64,
    pub query: String,
    pub extensions: Vec<String>,
    pub files: Vec<FileMatch>,
}

/// Joins the scan thread when the filesystem worker exits, including early returns.
pub struct ShutdownGuard(pub std::sync::Arc<SearchHub>);

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

/// Background content scan. The filesystem command thread only posts jobs.
pub struct SearchHub {
    jobs: Mutex<Option<SearchJob>>,
    signal: Condvar,
    cache: Mutex<TextCache>,
    stop: AtomicBool,
    thread: Mutex<Option<JoinHandle<()>>>,
}

enum ScanStep {
    Done,
    Replaced(SearchJob),
}

impl SearchHub {
    pub fn spawn(
        results: std::sync::mpsc::Sender<Reply>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> std::sync::Arc<Self> {
        let hub = std::sync::Arc::new(Self {
            jobs: Mutex::new(None),
            signal: Condvar::new(),
            cache: Mutex::new(TextCache::default()),
            stop: AtomicBool::new(false),
            thread: Mutex::new(None),
        });
        let worker = std::sync::Arc::clone(&hub);
        let handle = std::thread::spawn(move || worker.run(&results, wake));
        if let Ok(mut slot) = hub.thread.lock() {
            *slot = Some(handle);
        }
        hub
    }

    pub fn submit(&self, job: SearchJob) {
        if let Ok(mut guard) = self.jobs.lock() {
            *guard = Some(job);
            self.signal.notify_one();
        }
    }

    /// Drops cached hits for `path` and its descendants.
    pub fn forget(&self, path: &Path) -> Result<(), super::FsError> {
        self.cache.lock().map_err(|_| poisoned())?.forget(path);
        Ok(())
    }

    /// Drops every stored query.
    pub fn clear(&self) -> Result<(), super::FsError> {
        self.cache.lock().map_err(|_| poisoned())?.clear();
        Ok(())
    }

    /// Clears the cache and waits for the scan thread to exit.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
        self.signal.notify_all();
        if let Ok(mut slot) = self.thread.lock()
            && let Some(handle) = slot.take()
        {
            let _joined = handle.join();
        }
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }

    fn run(self: &std::sync::Arc<Self>, results: &std::sync::mpsc::Sender<Reply>, wake: impl Fn()) {
        while !self.stop.load(Ordering::Acquire) {
            let Some(mut job) = self.wait_job() else {
                break;
            };
            while !self.stop.load(Ordering::Acquire) {
                match self.scan(&job, results, &wake) {
                    ScanStep::Done => break,
                    ScanStep::Replaced(next) => job = next,
                }
            }
        }
    }

    fn wait_job(&self) -> Option<SearchJob> {
        let Ok(mut guard) = self.jobs.lock() else {
            return None;
        };
        loop {
            if self.stop.load(Ordering::Acquire) {
                return None;
            }
            if let Some(job) = guard.take() {
                return Some(job);
            }
            guard = self.signal.wait(guard).ok()?;
        }
    }

    fn poll_job(&self) -> Option<SearchJob> {
        self.jobs.lock().ok().and_then(|mut guard| guard.take())
    }

    fn scan(
        &self,
        job: &SearchJob,
        results: &std::sync::mpsc::Sender<Reply>,
        wake: &impl Fn(),
    ) -> ScanStep {
        if job.query.is_empty() {
            return finish(results, wake, job.token, Vec::new(), 0);
        }
        let (indexed, rest) = split_files(&job.files, &job.extensions);
        if let Some(newer) = self.poll_job() {
            return ScanStep::Replaced(newer);
        }
        let epoch = match self.cache.lock() {
            Ok(cache) => cache.epoch,
            Err(_) => return finish(results, wake, job.token, Vec::new(), 0),
        };
        if fully_cached(&self.cache, &job.query, &indexed, &rest) {
            let hits = collect_cached(&self.cache, &job.query, &indexed, &rest);
            return finish(results, wake, job.token, hits, 0);
        }
        if send(
            results,
            wake,
            Reply::TextMatches {
                token: job.token,
                hits: Vec::new(),
                scanning: true,
                done: false,
                files_read: 0,
            },
        )
        .is_err()
        {
            return ScanStep::Done;
        }
        let mut sent = 0_usize;
        let mut files_read = 0_usize;
        for (phase_indexed, files) in [(true, &indexed), (false, &rest)] {
            for file in files {
                if let Some(newer) = self.poll_job() {
                    return ScanStep::Replaced(newer);
                }
                if self.stop.load(Ordering::Acquire) {
                    return ScanStep::Done;
                }
                let Ok(read) = self.hits_for(job, file, phase_indexed, epoch) else {
                    return finish(results, wake, job.token, Vec::new(), files_read);
                };
                files_read += usize::from(read.miss);
                if sent >= MAX_HITS || read.hits.is_empty() {
                    continue;
                }
                let room = MAX_HITS.saturating_sub(sent);
                let mut hits = read.hits;
                hits.truncate(room);
                sent += hits.len();
                if send(
                    results,
                    wake,
                    Reply::TextMatches {
                        token: job.token,
                        hits,
                        scanning: true,
                        done: false,
                        files_read,
                    },
                )
                .is_err()
                {
                    return ScanStep::Done;
                }
                files_read = 0;
            }
        }
        finish(results, wake, job.token, Vec::new(), files_read)
    }

    fn hits_for(
        &self,
        job: &SearchJob,
        file: &FileMatch,
        indexed: bool,
        epoch: u64,
    ) -> Result<ReadOutcome, ()> {
        let cached = self
            .cache
            .lock()
            .map_err(|_| ())?
            .get(&job.query, &file.path);
        if let Some(hits) = cached {
            return Ok(ReadOutcome { hits, miss: false });
        }
        let hits = search_file(&file.path, &file.relative, &job.query, indexed);
        let mut cache = self.cache.lock().map_err(|_| ())?;
        if cache.epoch == epoch {
            cache.insert(&job.query, file.path.clone(), hits.clone());
        }
        drop(cache);
        Ok(ReadOutcome { hits, miss: true })
    }
}

struct ReadOutcome {
    hits: Vec<TextHit>,
    miss: bool,
}

fn fully_cached(
    cache: &Mutex<TextCache>,
    query: &str,
    indexed: &[FileMatch],
    rest: &[FileMatch],
) -> bool {
    let Ok(cache) = cache.lock() else {
        return false;
    };
    indexed
        .iter()
        .chain(rest)
        .all(|file| cache.get(query, &file.path).is_some())
}

fn collect_cached(
    cache: &Mutex<TextCache>,
    query: &str,
    indexed: &[FileMatch],
    rest: &[FileMatch],
) -> Vec<TextHit> {
    let Ok(cache) = cache.lock() else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    for file in indexed.iter().chain(rest) {
        if hits.len() >= MAX_HITS {
            break;
        }
        if let Some(mut found) = cache.get(query, &file.path) {
            let room = MAX_HITS.saturating_sub(hits.len());
            found.truncate(room);
            hits.extend(found);
        }
    }
    hits
}

fn finish(
    results: &std::sync::mpsc::Sender<Reply>,
    wake: &impl Fn(),
    token: u64,
    hits: Vec<TextHit>,
    files_read: usize,
) -> ScanStep {
    let _sent = send(
        results,
        wake,
        Reply::TextMatches {
            token,
            hits,
            scanning: false,
            done: true,
            files_read,
        },
    );
    ScanStep::Done
}

fn send(
    results: &std::sync::mpsc::Sender<Reply>,
    wake: &impl Fn(),
    reply: Reply,
) -> Result<(), ()> {
    results.send(reply).map_err(|_| ())?;
    wake();
    Ok(())
}

fn split_files(files: &[FileMatch], extensions: &[String]) -> (Vec<FileMatch>, Vec<FileMatch>) {
    let mut indexed = Vec::new();
    let mut rest = Vec::new();
    for file in files {
        if extension_indexed(&file.path, extensions) {
            indexed.push(file.clone());
        } else {
            rest.push(file.clone());
        }
    }
    indexed.sort_by(|a, b| a.relative.cmp(&b.relative));
    rest.sort_by(|a, b| a.relative.cmp(&b.relative));
    (indexed, rest)
}

fn extension_indexed(path: &Path, extensions: &[String]) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            extensions
                .iter()
                .any(|known| known.eq_ignore_ascii_case(ext))
        })
}

fn search_file(path: &Path, relative: &str, query: &str, indexed: bool) -> Vec<TextHit> {
    let Some(text) = read_searchable(path) else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    let mut start = 0_usize;
    while hits.len() < MAX_HITS_PER_FILE {
        let Some(found) = text[start..].find(query) else {
            break;
        };
        let byte = start + found;
        let end = byte + query.len();
        let (line, preview) = line_preview(&text, byte);
        hits.push(TextHit {
            path: path.to_path_buf(),
            relative: relative.to_owned(),
            line,
            preview,
            byte,
            end,
            indexed,
        });
        start = end;
    }
    hits
}

fn read_searchable(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES || meta.len() == 0 {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(1024)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.contains(&0) {
        return None;
    }
    file.read_to_end(&mut bytes).ok()?;
    String::from_utf8(bytes).ok()
}

fn line_preview(text: &str, byte: usize) -> (u32, String) {
    let start = text[..byte].rfind('\n').map_or(0, |index| index + 1);
    let end = text[byte..]
        .find(['\n', '\r'])
        .map_or(text.len(), |index| byte + index);
    let line =
        u32::try_from(text[..start].bytes().filter(|b| *b == b'\n').count()).unwrap_or(u32::MAX);
    let mut preview: String = text[start..end]
        .trim()
        .chars()
        .take(PREVIEW_CHARS)
        .collect();
    if text[start..end].trim().chars().nth(PREVIEW_CHARS).is_some() {
        preview.push('…');
    }
    (line, preview)
}

fn covers(root: &Path, candidate: &Path) -> bool {
    candidate == root || candidate.starts_with(root)
}

fn covers_canonical(canonical: Option<&Path>, candidate: &Path) -> bool {
    canonical.is_some_and(|root| candidate == root || candidate.starts_with(root))
}

fn poisoned() -> super::FsError {
    super::FsError::Io(std::io::Error::other("Search cache lock poisoned"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_hits_skip_binary_and_keep_line_bytes() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let php = dir.path().join("Widget.php");
        std::fs::write(&php, "<?php\nclass Widget { const NEEDLE = 1; }\n")?;
        let hits = search_file(&php, "Widget.php", "NEEDLE", true);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].indexed);
        assert_eq!(hits[0].line, 1);
        assert_eq!(hits[0].byte, "<?php\nclass Widget { const ".len());
        assert!(hits[0].preview.contains("NEEDLE"));
        let blob = dir.path().join("blob.bin");
        std::fs::write(&blob, b"NEEDLE\0MORE")?;
        assert_eq!(search_file(&blob, "blob.bin", "NEEDLE", false).len(), 0);
        Ok(())
    }
}
