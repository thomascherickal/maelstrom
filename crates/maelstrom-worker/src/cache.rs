//! Manage downloading, extracting, and storing of artifacts specified by jobs.

use bytesize::ByteSize;
use maelstrom_base::{JobId, Sha256Digest};
use maelstrom_util::{
    config::common::CacheSize,
    heap::{Heap, HeapDeps, HeapIndex},
    root::RootBuf,
};
use slog::{debug, Logger};
use std::{
    cmp::Ordering,
    collections::{hash_map::Entry as HashEntry, HashMap},
    fmt, fs, mem,
    num::NonZeroU32,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    thread,
};

/// Dependencies that [Cache] has on the file system.
pub trait Fs {
    /// Return a random u64. This is used for creating unique path names in the directory removal
    /// code path.
    fn rand_u64(&mut self) -> u64;

    /// Return true if a file (or directory, or symlink, etc.) exists with the given path, and
    /// false otherwise. Panic on file system error.
    fn file_exists(&self, path: &Path) -> bool;

    /// Rename `source` to `destination`. Panic on file system error. Assume that all intermediate
    /// directories exist for `destination`, and that `source` and `destination` are on the same
    /// file system.
    fn rename(&mut self, source: &Path, destination: &Path);

    /// Remove `path`, and if `path` is a directory, all descendants of `path`. Do this on a
    /// separate thread. Panic on file system error.
    fn remove_recursively_on_thread(&mut self, path: PathBuf);

    /// Ensure `path` exists and is a directory. If it doesn't exist, recusively ensure its parent exists,
    /// then create it. Panic on file system error or if `path` or any of its ancestors aren't
    /// directories.
    fn mkdir_recursively(&mut self, path: &Path);

    /// Return and iterator that will yield all of the children of a directory. Panic on file
    /// system error or if `path` doesn't exist or isn't a directory.
    fn read_dir(&self, path: &Path) -> Box<dyn Iterator<Item = PathBuf>>;
}

/// The standard implementation of CacheFs that uses [std] and [rand].
pub struct StdFs;

impl Fs for StdFs {
    fn rand_u64(&mut self) -> u64 {
        rand::random()
    }

    fn file_exists(&self, path: &Path) -> bool {
        path.try_exists().unwrap()
    }

    fn rename(&mut self, source: &Path, destination: &Path) {
        fs::rename(source, destination).unwrap()
    }

    fn remove_recursively_on_thread(&mut self, path: PathBuf) {
        thread::spawn(move || {
            if path.is_dir() {
                fs::remove_dir_all(path).unwrap()
            } else {
                fs::remove_file(path).unwrap();
            }
        });
    }

    fn mkdir_recursively(&mut self, path: &Path) {
        fs::create_dir_all(path).unwrap();
    }

    fn read_dir(&self, path: &Path) -> Box<dyn Iterator<Item = PathBuf>> {
        Box::new(fs::read_dir(path).unwrap().map(|de| de.unwrap().path()))
    }
}

/// Type returned from [Cache::get_artifact].
#[derive(Clone, Debug, PartialEq)]
pub enum GetArtifact {
    /// The artifact in the cache. The caller has been given a reference that must later be
    /// released by calling [Cache::decrement_ref_count]. The provided [PathBuf] contains the
    /// location of the artifact.
    Success(PathBuf),

    /// The artifact is not in the cache and is currently being retrieved. There is nothing for
    /// the caller to do other than wait. The caller's [JobId] will be returned at some point from
    /// a call to [Cache::got_artifact_success] or [Cache::got_artifact_failure].
    Wait,

    /// The artifact is not in the cache but is not currently being retrieved. It's caller's
    /// responsibility to start the retrieval process. The artifact should be put in the provided
    /// [PathBuf]. The caller's [JobId] will be returned at some point from a call to
    /// [Cache::got_artifact_success] or [Cache::got_artifact_failure].
    Get(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, strum::EnumIter)]
pub enum EntryKind {
    Blob,
    BottomFsLayer,
    UpperFsLayer,
}

impl EntryKind {
    fn iter() -> impl DoubleEndedIterator<Item = Self> {
        <Self as strum::IntoEnumIterator>::iter()
    }
}

impl fmt::Display for EntryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blob => write!(f, "blob"),
            Self::BottomFsLayer => write!(f, "bottom_fs_layer"),
            Self::UpperFsLayer => write!(f, "upper_fs_layer"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub kind: EntryKind,
    pub digest: Sha256Digest,
}

impl Key {
    pub fn new(kind: EntryKind, digest: Sha256Digest) -> Self {
        Self { kind, digest }
    }
}

/// An entry for a specific [Sha256Digest] in the [Cache]'s hash table. There is one of these for
/// every subdirectory in the `sha256` subdirectory of the [Cache]'s root directory.
enum Entry {
    /// The artifact is being downloaded, extracted, and having its checksum validated. There is
    /// probably a subdirectory for this [Sha256Digest], but there might not yet be one, depending
    /// on how far along the extraction process is.
    DownloadingAndExtracting(Vec<JobId>),

    /// The artifact has been successfully downloaded and extracted, and the subdirectory is
    /// currently being used by at least one job. We reference count this state since there may be
    /// multiple jobs using the same artifact.
    InUse {
        bytes_used: u64,
        ref_count: NonZeroU32,
    },

    /// The artifact has been successfully downloaded and extracted, but no jobs are
    /// currently using it. The `priority` is provided by [Cache] and is used by the [Heap] to
    /// determine which entry should be removed first when freeing up space.
    InHeap {
        bytes_used: u64,
        priority: u64,
        heap_index: HeapIndex,
    },
}

/// An implementation of the "newtype" pattern so that we can implement [HeapDeps] on a [HashMap].
#[derive(Default)]
struct Map(HashMap<Key, Entry>);

impl Deref for Map {
    type Target = HashMap<Key, Entry>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Map {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl HeapDeps for Map {
    type Element = Key;

    fn is_element_less_than(&self, lhs: &Self::Element, rhs: &Self::Element) -> bool {
        let lhs_priority = match self.get(lhs) {
            Some(Entry::InHeap { priority, .. }) => *priority,
            _ => panic!("Element should be in heap"),
        };
        let rhs_priority = match self.get(rhs) {
            Some(Entry::InHeap { priority, .. }) => *priority,
            _ => panic!("Element should be in heap"),
        };
        lhs_priority.cmp(&rhs_priority) == Ordering::Less
    }

    fn update_index(&mut self, elem: &Self::Element, idx: HeapIndex) {
        match self.get_mut(elem) {
            Some(Entry::InHeap { heap_index, .. }) => *heap_index = idx,
            _ => panic!("Element should be in heap"),
        };
    }
}

pub struct CacheDir;

/// Manage a directory of downloaded, extracted artifacts. Coordinate fetching of these artifacts,
/// and removing them when they are no longer in use and the amount of space used by the directory
/// has grown too large.
pub struct Cache<FsT> {
    fs: FsT,
    root: PathBuf,
    entries: Map,
    heap: Heap<Map>,
    next_priority: u64,
    bytes_used: u64,
    bytes_used_target: u64,
    log: Logger,
}

impl<FsT: Fs> Cache<FsT> {
    /// Create a new [Cache] rooted at `root`. The directory `root` and all necessary ancestors
    /// will be created, along with `{root}/removing` and `{root}/{kind}/sha256`. Any pre-existing
    /// entries in `{root}/removing` and `{root}/{kind}/sha256` will be removed. That implies that
    /// the [Cache] doesn't currently keep data stored across invocations.
    ///
    /// `bytes_used_target` is the goal on-disk size for the cache. The cache will periodically grow
    /// larger than this size, but then shrink back down to this size. Ideally, the cache would use
    /// this as a hard upper bound, but that's not how it currently works.
    pub fn new(mut fs: FsT, root: RootBuf<CacheDir>, size: CacheSize, log: Logger) -> Self {
        let root = root.into_path_buf();
        let mut path = root.clone();

        path.push("removing");
        fs.mkdir_recursively(&path);
        for child in fs.read_dir(&path) {
            fs.remove_recursively_on_thread(child);
        }
        path.pop();

        for kind in EntryKind::iter() {
            let mut path = path.clone();
            path.push(kind.to_string());
            path.push("sha256");
            if fs.file_exists(&path) {
                Self::remove_in_background(&mut fs, &root, &path);
            }
            fs.mkdir_recursively(&path);
        }

        Cache {
            fs,
            root,
            entries: Map::default(),
            heap: Heap::default(),
            next_priority: 0,
            bytes_used: 0,
            bytes_used_target: size.as_bytes(),
            log,
        }
    }

    /// Attempt to fetch `artifact` from the cache. See [GetArtifact] for the meaning of the return
    /// values.
    pub fn get_artifact(
        &mut self,
        kind: EntryKind,
        digest: Sha256Digest,
        jid: JobId,
    ) -> GetArtifact {
        let key = Key::new(kind, digest);
        let cache_path = Self::cache_path(&self.root, &key);
        match self.entries.entry(key) {
            HashEntry::Vacant(entry) => {
                entry.insert(Entry::DownloadingAndExtracting(vec![jid]));
                GetArtifact::Get(cache_path)
            }
            HashEntry::Occupied(entry) => {
                let entry = entry.into_mut();
                match entry {
                    Entry::DownloadingAndExtracting(jobs) => {
                        jobs.push(jid);
                        GetArtifact::Wait
                    }
                    Entry::InUse { ref_count, .. } => {
                        *ref_count = ref_count.checked_add(1).unwrap();
                        GetArtifact::Success(cache_path)
                    }
                    Entry::InHeap {
                        bytes_used,
                        heap_index,
                        ..
                    } => {
                        let heap_index = *heap_index;
                        *entry = Entry::InUse {
                            ref_count: NonZeroU32::new(1).unwrap(),
                            bytes_used: *bytes_used,
                        };
                        self.heap.remove(&mut self.entries, heap_index);
                        GetArtifact::Success(cache_path)
                    }
                }
            }
        }
    }

    /// Notify the cache that an artifact fetch has failed. The returned vector lists the jobs that
    /// are affected and that need to be canceled.
    pub fn got_artifact_failure(&mut self, kind: EntryKind, digest: &Sha256Digest) -> Vec<JobId> {
        let key = Key::new(kind, digest.clone());
        let Some(Entry::DownloadingAndExtracting(jobs)) = self.entries.remove(&key) else {
            panic!("Got got_artifact in unexpected state");
        };
        let cache_path = Self::cache_path(&self.root, &key);
        if self.fs.file_exists(&cache_path) {
            Self::remove_in_background(&mut self.fs, &self.root, &cache_path);
        }
        jobs
    }

    /// Notify the cache that an artifact fetch has successfully completed. The returned vector
    /// lists the jobs that are affected, and the path they can use to access the artifact.
    pub fn got_artifact_success(
        &mut self,
        kind: EntryKind,
        digest: &Sha256Digest,
        bytes_used: u64,
    ) -> (PathBuf, Vec<JobId>) {
        let key = Key::new(kind, digest.clone());
        let entry = self
            .entries
            .get_mut(&key)
            .expect("Got DownloadingAndExtracting in unexpected state");
        let Entry::DownloadingAndExtracting(jobs) = entry else {
            panic!("Got DownloadingAndExtracting in unexpected state");
        };
        let ref_count = jobs.len().try_into().unwrap();
        let jobs = mem::take(jobs);
        // Reference count must be > 0 since we don't allow cancellation of gets.
        *entry = Entry::InUse {
            bytes_used,
            ref_count: NonZeroU32::new(ref_count).unwrap(),
        };
        self.bytes_used = self.bytes_used.checked_add(bytes_used).unwrap();
        debug!(self.log, "cache added artifact";
            "kind" => ?kind,
            "digest" => %digest,
            "artifact_bytes_used" => %ByteSize::b(bytes_used),
            "entries" => %self.entries.len(),
            "bytes_used" => %ByteSize::b(self.bytes_used),
            "byte_used_target" => %ByteSize::b(self.bytes_used_target)
        );
        self.possibly_remove_some();
        (Self::cache_path(&self.root, &key), jobs)
    }

    /// Notify the cache that a reference to an artifact is no longer needed.
    pub fn decrement_ref_count(&mut self, kind: EntryKind, digest: &Sha256Digest) {
        let key = Key::new(kind, digest.clone());
        let entry = self
            .entries
            .get_mut(&key)
            .expect("Got decrement_ref_count in unexpected state");
        let Entry::InUse {
            bytes_used,
            ref_count,
        } = entry
        else {
            panic!("Got decrement_ref_count with existing zero reference count");
        };
        match NonZeroU32::new(ref_count.get() - 1) {
            Some(new_ref_count) => *ref_count = new_ref_count,
            None => {
                *entry = Entry::InHeap {
                    bytes_used: *bytes_used,
                    priority: self.next_priority,
                    heap_index: HeapIndex::default(),
                };
                self.heap.push(&mut self.entries, key.clone());
                self.next_priority = self.next_priority.checked_add(1).unwrap();
                self.possibly_remove_some();
            }
        }
    }

    /// Remove all files and directories rooted in `source` in a separate thread.
    fn remove_in_background(fs: &mut impl Fs, root: &Path, source: &Path) {
        let mut target = root.to_owned();
        target.push("removing");
        loop {
            let key = fs.rand_u64();
            target.push(format!("{key:016x}"));
            if !fs.file_exists(&target) {
                break;
            } else {
                target.pop();
            }
        }
        fs.rename(source, &target);
        fs.remove_recursively_on_thread(target);
    }

    /// Return the directory path for the artifact referenced by `digest`.
    fn cache_path(root: &Path, key: &Key) -> PathBuf {
        let mut path = root.to_owned();
        path.push(key.kind.to_string());
        path.push("sha256");
        path.push(key.digest.to_string());
        path
    }

    /// Check to see if the cache is over its goal size, and if so, try to remove the least
    /// recently used artifacts.
    fn possibly_remove_some(&mut self) {
        while self.bytes_used > self.bytes_used_target {
            let Some(key) = self.heap.pop(&mut self.entries) else {
                break;
            };
            let Some(Entry::InHeap { bytes_used, .. }) = self.entries.remove(&key) else {
                panic!("Entry popped off of heap was in unexpected state");
            };
            Self::remove_in_background(
                &mut self.fs,
                &self.root,
                &Self::cache_path(&self.root, &key),
            );
            self.bytes_used = self.bytes_used.checked_sub(bytes_used).unwrap();
            debug!(self.log, "cache removed artifact";
                "key" => ?key,
                "artifact_bytes_used" => %ByteSize::b(bytes_used),
                "entries" => %self.entries.len(),
                "bytes_used" => %ByteSize::b(self.bytes_used),
                "byte_used_target" => %ByteSize::b(self.bytes_used_target)
            );
        }
    }
}

/*  _            _
 * | |_ ___  ___| |_ ___
 * | __/ _ \/ __| __/ __|
 * | ||  __/\__ \ |_\__ \
 *  \__\___||___/\__|___/
 *  FIGLET: tests
 */

#[cfg(test)]
mod tests {
    use super::*;
    use itertools::Itertools;
    use maelstrom_test::*;
    use slog::{o, Discard};
    use std::{cell::RefCell, collections::HashSet, rc::Rc};
    use TestMessage::*;

    #[derive(Clone, Debug, PartialEq)]
    enum TestMessage {
        FileExists(PathBuf),
        Rename(PathBuf, PathBuf),
        RemoveRecursively(PathBuf),
        MkdirRecursively(PathBuf),
        ReadDir(PathBuf),
    }

    #[derive(Default)]
    struct TestFs {
        messages: Rc<RefCell<Vec<TestMessage>>>,
        existing_files: HashSet<PathBuf>,
        directories: HashMap<PathBuf, Vec<PathBuf>>,
        last_random_number: u64,
    }

    impl Fs for TestFs {
        fn rand_u64(&mut self) -> u64 {
            self.last_random_number += 1;
            self.last_random_number
        }

        fn file_exists(&self, path: &Path) -> bool {
            self.messages.borrow_mut().push(FileExists(path.to_owned()));
            self.existing_files.contains(path)
        }

        fn rename(&mut self, source: &Path, destination: &Path) {
            self.messages
                .borrow_mut()
                .push(Rename(source.to_owned(), destination.to_owned()));
        }

        fn remove_recursively_on_thread(&mut self, path: PathBuf) {
            self.messages
                .borrow_mut()
                .push(RemoveRecursively(path.to_owned()));
        }

        fn mkdir_recursively(&mut self, path: &Path) {
            self.messages
                .borrow_mut()
                .push(MkdirRecursively(path.to_owned()));
        }

        fn read_dir(&self, path: &Path) -> Box<dyn Iterator<Item = PathBuf>> {
            self.messages.borrow_mut().push(ReadDir(path.to_owned()));
            Box::new(
                self.directories
                    .get(path)
                    .unwrap_or(&vec![])
                    .clone()
                    .into_iter(),
            )
        }
    }

    struct Fixture {
        messages: Rc<RefCell<Vec<TestMessage>>>,
        cache: Cache<TestFs>,
    }

    impl Fixture {
        fn new_with_fs_and_clear_messages(test_cache_fs: TestFs, bytes_used_target: u64) -> Self {
            let mut fixture = Fixture::new(test_cache_fs, bytes_used_target);
            fixture.clear_messages();
            fixture
        }

        fn new_and_clear_messages(bytes_used_target: u64) -> Self {
            Self::new_with_fs_and_clear_messages(TestFs::default(), bytes_used_target)
        }

        fn new(test_cache_fs: TestFs, bytes_used_target: u64) -> Self {
            let messages = test_cache_fs.messages.clone();
            let cache = Cache::new(
                test_cache_fs,
                "/z".parse().unwrap(),
                ByteSize::b(bytes_used_target).into(),
                Logger::root(Discard, o!()),
            );
            Fixture { messages, cache }
        }

        fn expect_messages_in_any_order(&mut self, expected: Vec<TestMessage>) {
            let mut messages = self.messages.borrow_mut();
            for perm in expected.clone().into_iter().permutations(expected.len()) {
                if perm == *messages {
                    messages.clear();
                    return;
                }
            }
            panic!(
                "Expected messages didn't match actual messages in any order.\n{}",
                colored_diff::PrettyDifference {
                    expected: &format!("{:#?}", expected),
                    actual: &format!("{:#?}", messages)
                }
            );
        }

        fn expect_messages_in_specific_order(&mut self, expected: Vec<TestMessage>) {
            assert!(
                *self.messages.borrow() == expected,
                "Expected messages didn't match actual messages in specific order.\n{}",
                colored_diff::PrettyDifference {
                    expected: &format!("{:#?}", expected),
                    actual: &format!("{:#?}", self.messages.borrow())
                }
            );
            self.clear_messages();
        }

        fn clear_messages(&mut self) {
            self.messages.borrow_mut().clear();
        }

        fn get_artifact(&mut self, digest: Sha256Digest, jid: JobId, expected: GetArtifact) {
            let result = self.cache.get_artifact(EntryKind::Blob, digest, jid);
            assert_eq!(result, expected);
            self.expect_messages_in_any_order(vec![]);
        }

        fn get_artifact_ign(&mut self, digest: Sha256Digest, jid: JobId) {
            self.cache.get_artifact(EntryKind::Blob, digest, jid);
            self.expect_messages_in_any_order(vec![]);
        }

        fn got_artifact_success(
            &mut self,
            digest: Sha256Digest,
            bytes_used: u64,
            expected: (PathBuf, Vec<JobId>),
            expected_fs_operations: Vec<TestMessage>,
        ) {
            let result = self
                .cache
                .got_artifact_success(EntryKind::Blob, &digest, bytes_used);
            assert_eq!(result, expected);
            self.expect_messages_in_any_order(expected_fs_operations);
        }

        fn got_artifact_failure(
            &mut self,
            digest: Sha256Digest,
            expected: Vec<JobId>,
            expected_fs_operations: Vec<TestMessage>,
        ) {
            let result = self.cache.got_artifact_failure(EntryKind::Blob, &digest);
            assert_eq!(result, expected);
            self.expect_messages_in_any_order(expected_fs_operations);
        }

        fn got_artifact_success_ign(&mut self, digest: Sha256Digest, bytes_used: u64) {
            self.cache
                .got_artifact_success(EntryKind::Blob, &digest, bytes_used);
            self.clear_messages();
        }

        fn decrement_ref_count(&mut self, digest: Sha256Digest, expected: Vec<TestMessage>) {
            self.cache.decrement_ref_count(EntryKind::Blob, &digest);
            self.expect_messages_in_any_order(expected);
        }

        fn decrement_ref_count_ign(&mut self, digest: Sha256Digest) {
            self.cache.decrement_ref_count(EntryKind::Blob, &digest);
            self.clear_messages();
        }
    }

    #[test]
    fn get_request_for_empty() {
        let mut fixture = Fixture::new_and_clear_messages(1000);

        fixture.get_artifact(
            digest!(42),
            jid!(1),
            GetArtifact::Get(long_path!("/z/blob/sha256", 42)),
        );
        fixture.got_artifact_success(
            digest!(42),
            100,
            (long_path!("/z/blob/sha256", 42), vec![jid!(1)]),
            vec![],
        );
    }

    #[test]
    fn get_request_for_empty_larger_than_goal_ok_then_removes_on_decrement_ref_count() {
        let mut fixture = Fixture::new_and_clear_messages(1000);

        fixture.get_artifact_ign(digest!(42), jid!(1));
        fixture.got_artifact_success(
            digest!(42),
            10000,
            (long_path!("/z/blob/sha256", 42), vec![jid!(1)]),
            vec![],
        );

        fixture.decrement_ref_count(
            digest!(42),
            vec![
                FileExists(short_path!("/z/removing", 1)),
                Rename(
                    long_path!("/z/blob/sha256", 42),
                    short_path!("/z/removing", 1),
                ),
                RemoveRecursively(short_path!("/z/removing", 1)),
            ],
        );
    }

    #[test]
    fn cache_entries_are_removed_in_lru_order() {
        let mut fixture = Fixture::new_and_clear_messages(10);

        fixture.get_artifact_ign(digest!(1), jid!(1));
        fixture.got_artifact_success_ign(digest!(1), 4);
        fixture.decrement_ref_count(digest!(1), vec![]);

        fixture.get_artifact_ign(digest!(2), jid!(2));
        fixture.got_artifact_success_ign(digest!(2), 4);
        fixture.decrement_ref_count(digest!(2), vec![]);

        fixture.get_artifact_ign(digest!(3), jid!(3));
        fixture.got_artifact_success(
            digest!(3),
            4,
            (long_path!("/z/blob/sha256", 3), vec![jid!(3)]),
            vec![
                FileExists(short_path!("/z/removing", 1)),
                Rename(
                    long_path!("/z/blob/sha256", 1),
                    short_path!("/z/removing", 1),
                ),
                RemoveRecursively(short_path!("/z/removing", 1)),
            ],
        );
        fixture.decrement_ref_count(digest!(3), vec![]);

        fixture.get_artifact_ign(digest!(4), jid!(4));
        fixture.got_artifact_success(
            digest!(4),
            4,
            (long_path!("/z/blob/sha256", 4), vec![jid!(4)]),
            vec![
                FileExists(short_path!("/z/removing", 2)),
                Rename(
                    long_path!("/z/blob/sha256", 2),
                    short_path!("/z/removing", 2),
                ),
                RemoveRecursively(short_path!("/z/removing", 2)),
            ],
        );
        fixture.decrement_ref_count(digest!(4), vec![]);
    }

    #[test]
    fn lru_order_augmented_by_last_use() {
        let mut fixture = Fixture::new_and_clear_messages(10);

        fixture.get_artifact_ign(digest!(1), jid!(1));
        fixture.got_artifact_success_ign(digest!(1), 3);

        fixture.get_artifact_ign(digest!(2), jid!(2));
        fixture.got_artifact_success_ign(digest!(2), 3);

        fixture.get_artifact_ign(digest!(3), jid!(3));
        fixture.got_artifact_success_ign(digest!(3), 3);

        fixture.decrement_ref_count(digest!(3), vec![]);
        fixture.decrement_ref_count(digest!(2), vec![]);
        fixture.decrement_ref_count(digest!(1), vec![]);

        fixture.get_artifact_ign(digest!(4), jid!(4));
        fixture.got_artifact_success(
            digest!(4),
            3,
            (long_path!("/z/blob/sha256", 4), vec![jid!(4)]),
            vec![
                FileExists(short_path!("/z/removing", 1)),
                Rename(
                    long_path!("/z/blob/sha256", 3),
                    short_path!("/z/removing", 1),
                ),
                RemoveRecursively(short_path!("/z/removing", 1)),
            ],
        );
    }

    #[test]
    fn multiple_get_requests_for_empty() {
        let mut fixture = Fixture::new_and_clear_messages(1000);

        fixture.get_artifact_ign(digest!(42), jid!(1));
        fixture.get_artifact(digest!(42), jid!(2), GetArtifact::Wait);
        fixture.get_artifact(digest!(42), jid!(3), GetArtifact::Wait);

        fixture.got_artifact_success(
            digest!(42),
            100,
            (
                long_path!("/z/blob/sha256", 42),
                vec![jid!(1), jid!(2), jid!(3)],
            ),
            vec![],
        );
    }

    #[test]
    fn multiple_get_requests_for_empty_larger_than_goal_remove_on_last_decrement() {
        let mut fixture = Fixture::new_and_clear_messages(1000);

        fixture.get_artifact_ign(digest!(42), jid!(1));
        fixture.get_artifact(digest!(42), jid!(2), GetArtifact::Wait);
        fixture.get_artifact(digest!(42), jid!(3), GetArtifact::Wait);

        fixture.got_artifact_success(
            digest!(42),
            10000,
            (
                long_path!("/z/blob/sha256", 42),
                vec![jid!(1), jid!(2), jid!(3)],
            ),
            vec![],
        );

        fixture.decrement_ref_count(digest!(42), vec![]);
        fixture.decrement_ref_count(digest!(42), vec![]);
        fixture.decrement_ref_count(
            digest!(42),
            vec![
                FileExists(short_path!("/z/removing", 1)),
                Rename(
                    long_path!("/z/blob/sha256", 42),
                    short_path!("/z/removing", 1),
                ),
                RemoveRecursively(short_path!("/z/removing", 1)),
            ],
        );
    }

    #[test]
    fn get_request_for_currently_used() {
        let mut fixture = Fixture::new_and_clear_messages(10);

        fixture.get_artifact_ign(digest!(42), jid!(1));
        fixture.got_artifact_success_ign(digest!(42), 100);

        fixture.get_artifact(
            digest!(42),
            jid!(1),
            GetArtifact::Success(long_path!("/z/blob/sha256", 42)),
        );

        fixture.decrement_ref_count(digest!(42), vec![]);
        fixture.decrement_ref_count(
            digest!(42),
            vec![
                FileExists(short_path!("/z/removing", 1)),
                Rename(
                    long_path!("/z/blob/sha256", 42),
                    short_path!("/z/removing", 1),
                ),
                RemoveRecursively(short_path!("/z/removing", 1)),
            ],
        );
    }

    #[test]
    fn get_request_for_cached_followed_by_big_get_does_not_evict_until_decrement_ref_count() {
        let mut fixture = Fixture::new_and_clear_messages(100);

        fixture.get_artifact_ign(digest!(42), jid!(1));
        fixture.got_artifact_success_ign(digest!(42), 10);
        fixture.decrement_ref_count_ign(digest!(42));

        fixture.get_artifact(
            digest!(42),
            jid!(2),
            GetArtifact::Success(long_path!("/z/blob/sha256", 42)),
        );
        fixture.get_artifact(
            digest!(43),
            jid!(3),
            GetArtifact::Get(long_path!("/z/blob/sha256", 43)),
        );
        fixture.got_artifact_success(
            digest!(43),
            100,
            (long_path!("/z/blob/sha256", 43), vec![jid!(3)]),
            vec![],
        );

        fixture.decrement_ref_count(
            digest!(42),
            vec![
                FileExists(short_path!("/z/removing", 1)),
                Rename(
                    long_path!("/z/blob/sha256", 42),
                    short_path!("/z/removing", 1),
                ),
                RemoveRecursively(short_path!("/z/removing", 1)),
            ],
        );
    }

    #[test]
    fn get_request_for_empty_with_download_and_extract_failure_and_no_files_created() {
        let mut fixture = Fixture::new_and_clear_messages(1000);

        fixture.get_artifact_ign(digest!(42), jid!(1));
        fixture.got_artifact_failure(
            digest!(42),
            vec![jid!(1)],
            vec![FileExists(long_path!("/z/blob/sha256", 42))],
        );
    }

    #[test]
    fn preexisting_directories_do_not_affect_get_request() {
        let mut test_cache_fs = TestFs::default();
        test_cache_fs
            .existing_files
            .insert(long_path!("/z/blob/sha256", 42));
        let mut fixture = Fixture::new_with_fs_and_clear_messages(test_cache_fs, 1000);

        fixture.get_artifact(
            digest!(42),
            jid!(1),
            GetArtifact::Get(long_path!("/z/blob/sha256", 42)),
        );
    }

    #[test]
    fn get_request_for_empty_with_download_and_extract_failure_and_files_created() {
        let mut test_cache_fs = TestFs::default();
        test_cache_fs
            .existing_files
            .insert(long_path!("/z/blob/sha256", 42));
        let mut fixture = Fixture::new_with_fs_and_clear_messages(test_cache_fs, 1000);

        fixture.get_artifact_ign(digest!(42), jid!(1));

        fixture.got_artifact_failure(
            digest!(42),
            vec![jid!(1)],
            vec![
                FileExists(long_path!("/z/blob/sha256", 42)),
                FileExists(short_path!("/z/removing", 1)),
                Rename(
                    long_path!("/z/blob/sha256", 42),
                    short_path!("/z/removing", 1),
                ),
                RemoveRecursively(short_path!("/z/removing", 1)),
            ],
        );
    }

    #[test]
    fn multiple_get_requests_for_empty_with_download_and_extract_failure() {
        let mut test_cache_fs = TestFs::default();
        test_cache_fs
            .existing_files
            .insert(long_path!("/z/blob/sha256", 42));
        let mut fixture = Fixture::new_with_fs_and_clear_messages(test_cache_fs, 1000);

        fixture.get_artifact_ign(digest!(42), jid!(1));
        fixture.get_artifact_ign(digest!(42), jid!(2));
        fixture.get_artifact_ign(digest!(42), jid!(3));

        fixture.got_artifact_failure(
            digest!(42),
            vec![jid!(1), jid!(2), jid!(3)],
            vec![
                FileExists(long_path!("/z/blob/sha256", 42)),
                FileExists(short_path!("/z/removing", 1)),
                Rename(
                    long_path!("/z/blob/sha256", 42),
                    short_path!("/z/removing", 1),
                ),
                RemoveRecursively(short_path!("/z/removing", 1)),
            ],
        );
    }

    #[test]
    fn get_after_error_retries() {
        let mut fixture = Fixture::new_and_clear_messages(1000);

        fixture.get_artifact_ign(digest!(42), jid!(1));

        fixture.got_artifact_failure(
            digest!(42),
            vec![jid!(1)],
            vec![FileExists(long_path!("/z/blob/sha256", 42))],
        );

        fixture.get_artifact(
            digest!(42),
            jid!(2),
            GetArtifact::Get(long_path!("/z/blob/sha256", 42)),
        );
    }

    #[test]
    fn rename_retries_until_unique_path_name() {
        let mut test_cache_fs = TestFs::default();
        test_cache_fs
            .existing_files
            .insert(long_path!("/z/blob/sha256", 42));
        test_cache_fs
            .existing_files
            .insert(short_path!("/z/removing", 1));
        test_cache_fs
            .existing_files
            .insert(short_path!("/z/removing", 2));
        test_cache_fs
            .existing_files
            .insert(short_path!("/z/removing", 3));
        let mut fixture = Fixture::new_with_fs_and_clear_messages(test_cache_fs, 1000);

        fixture.get_artifact_ign(digest!(42), jid!(1));

        fixture.got_artifact_failure(
            digest!(42),
            vec![jid!(1)],
            vec![
                FileExists(long_path!("/z/blob/sha256", 42)),
                FileExists(short_path!("/z/removing", 1)),
                FileExists(short_path!("/z/removing", 2)),
                FileExists(short_path!("/z/removing", 3)),
                FileExists(short_path!("/z/removing", 4)),
                Rename(
                    long_path!("/z/blob/sha256", 42),
                    short_path!("/z/removing", 4),
                ),
                RemoveRecursively(short_path!("/z/removing", 4)),
            ],
        );
    }

    #[test]
    fn new_ensures_directories_exist() {
        let mut fixture = Fixture::new(TestFs::default(), 1000);
        fixture.expect_messages_in_specific_order(vec![
            MkdirRecursively(path_buf!("/z/removing")),
            ReadDir(path_buf!("/z/removing")),
            FileExists(path_buf!("/z/blob/sha256")),
            MkdirRecursively(path_buf!("/z/blob/sha256")),
            FileExists(path_buf!("/z/bottom_fs_layer/sha256")),
            MkdirRecursively(path_buf!("/z/bottom_fs_layer/sha256")),
            FileExists(path_buf!("/z/upper_fs_layer/sha256")),
            MkdirRecursively(path_buf!("/z/upper_fs_layer/sha256")),
        ]);
    }

    #[test]
    fn new_restarts_old_removes() {
        let mut test_cache_fs = TestFs::default();
        test_cache_fs.directories.insert(
            path_buf!("/z/removing"),
            vec![
                short_path!("/z/removing", 10),
                short_path!("/z/removing", 20),
            ],
        );
        let mut fixture = Fixture::new(test_cache_fs, 1000);
        fixture.expect_messages_in_specific_order(vec![
            MkdirRecursively(path_buf!("/z/removing")),
            ReadDir(path_buf!("/z/removing")),
            RemoveRecursively(short_path!("/z/removing", 10)),
            RemoveRecursively(short_path!("/z/removing", 20)),
            FileExists(path_buf!("/z/blob/sha256")),
            MkdirRecursively(path_buf!("/z/blob/sha256")),
            FileExists(path_buf!("/z/bottom_fs_layer/sha256")),
            MkdirRecursively(path_buf!("/z/bottom_fs_layer/sha256")),
            FileExists(path_buf!("/z/upper_fs_layer/sha256")),
            MkdirRecursively(path_buf!("/z/upper_fs_layer/sha256")),
        ]);
    }

    #[test]
    fn new_removes_old_sha256_if_it_exists() {
        let mut test_cache_fs = TestFs::default();
        test_cache_fs
            .existing_files
            .insert(path_buf!("/z/blob/sha256"));
        test_cache_fs
            .existing_files
            .insert(path_buf!("/z/bottom_fs_layer/sha256"));
        test_cache_fs
            .existing_files
            .insert(path_buf!("/z/upper_fs_layer/sha256"));
        let mut fixture = Fixture::new(test_cache_fs, 1000);
        fixture.expect_messages_in_specific_order(vec![
            MkdirRecursively(path_buf!("/z/removing")),
            ReadDir(path_buf!("/z/removing")),
            FileExists(path_buf!("/z/blob/sha256")),
            FileExists(short_path!("/z/removing", 1)),
            Rename(path_buf!("/z/blob/sha256"), short_path!("/z/removing", 1)),
            RemoveRecursively(short_path!("/z/removing", 1)),
            MkdirRecursively(path_buf!("/z/blob/sha256")),
            FileExists(path_buf!("/z/bottom_fs_layer/sha256")),
            FileExists(short_path!("/z/removing", 2)),
            Rename(
                path_buf!("/z/bottom_fs_layer/sha256"),
                short_path!("/z/removing", 2),
            ),
            RemoveRecursively(short_path!("/z/removing", 2)),
            MkdirRecursively(path_buf!("/z/bottom_fs_layer/sha256")),
            FileExists(path_buf!("/z/upper_fs_layer/sha256")),
            FileExists(short_path!("/z/removing", 3)),
            Rename(
                path_buf!("/z/upper_fs_layer/sha256"),
                short_path!("/z/removing", 3),
            ),
            RemoveRecursively(short_path!("/z/removing", 3)),
            MkdirRecursively(path_buf!("/z/upper_fs_layer/sha256")),
        ]);
    }
}
