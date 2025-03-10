mod alternative_mains;
pub mod artifacts;
pub mod config;
mod deps;
mod introspect_driver;
pub mod metadata;
pub mod test_listing;
pub mod ui;
pub mod visitor;

#[cfg(test)]
mod tests;

pub use deps::*;

use anyhow::Result;
use artifacts::GeneratedArtifacts;
use clap::{Args, Command};
use introspect_driver::{DefaultIntrospectDriver, IntrospectDriver};
use maelstrom_base::{ArtifactType, JobRootOverlay, Sha256Digest, Timeout, Utf8PathBuf};
use maelstrom_client::{spec::JobSpec, ClientBgProcess, ProjectDir, StateDir};
use maelstrom_util::{
    config::common::LogLevel, config::Config, fs::Fs, process::ExitCode, root::Root,
};
use metadata::{AllMetadata, TestMetadata};
use slog::Drain as _;
use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsString,
    fmt::Debug,
    io::{self, IsTerminal as _},
    str,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use test_listing::TestListingStore;
use ui::{Ui, UiSender, UiSenderWriteAdapter};
use visitor::{JobStatusTracker, JobStatusVisitor};

#[derive(Debug)]
pub enum ListAction {
    ListTests,
}

type TestListing<TestCollectorT> = test_listing::TestListing<
    <TestCollectorT as CollectTests>::ArtifactKey,
    <TestCollectorT as CollectTests>::CaseMetadata,
>;

type CaseIter<CaseMetadataT> = <Vec<(String, CaseMetadataT)> as IntoIterator>::IntoIter;

type ArtifactM<MainAppDepsT> =
    <<MainAppDepsT as MainAppDeps>::TestCollector as CollectTests>::Artifact;

type CaseMetadataM<MainAppDepsT> =
    <<MainAppDepsT as MainAppDeps>::TestCollector as CollectTests>::CaseMetadata;

type ArtifactKeyM<MainAppDepsT> =
    <<MainAppDepsT as MainAppDeps>::TestCollector as CollectTests>::ArtifactKey;

type PackageM<MainAppDepsT> =
    <<MainAppDepsT as MainAppDeps>::TestCollector as CollectTests>::Package;

type CollectOptionsM<MainAppDepsT> =
    <<MainAppDepsT as MainAppDeps>::TestCollector as CollectTests>::Options;

type TestFilterM<MainAppDepsT> =
    <<MainAppDepsT as MainAppDeps>::TestCollector as CollectTests>::TestFilter;

type BuildHandleM<MainAppDepsT> =
    <<MainAppDepsT as MainAppDeps>::TestCollector as CollectTests>::BuildHandle;

type ArtifactStreamM<MainAppDepsT> =
    <<MainAppDepsT as MainAppDeps>::TestCollector as CollectTests>::ArtifactStream;

/// A collection of objects that are used while enqueuing jobs. This is useful as a separate object
/// since it can contain things which live longer than the scoped threads and thus can be shared
/// among them.
///
/// This object is separate from `MainAppState` because it is lent to `JobQueuing`
struct JobQueuingState<TestCollectorT: CollectTests> {
    packages: BTreeMap<TestCollectorT::PackageId, TestCollectorT::Package>,
    filter: TestCollectorT::TestFilter,
    stderr_color: bool,
    tracker: Arc<JobStatusTracker>,
    jobs_queued: AtomicU64,
    test_metadata: AllMetadata<TestCollectorT::TestFilter>,
    expected_job_count: u64,
    test_listing: Arc<Mutex<Option<TestListing<TestCollectorT>>>>,
    list_action: Option<ListAction>,
    collector_options: TestCollectorT::Options,
}

impl<TestCollectorT: CollectTests> JobQueuingState<TestCollectorT> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        packages: BTreeMap<TestCollectorT::PackageId, TestCollectorT::Package>,
        filter: TestCollectorT::TestFilter,
        stderr_color: bool,
        test_metadata: AllMetadata<TestCollectorT::TestFilter>,
        test_listing: TestListing<TestCollectorT>,
        list_action: Option<ListAction>,
        collector_options: TestCollectorT::Options,
    ) -> Result<Self> {
        let expected_job_count = test_listing.expected_job_count(&filter);

        Ok(Self {
            packages,
            filter,
            stderr_color,
            tracker: Arc::new(JobStatusTracker::default()),
            jobs_queued: AtomicU64::new(0),
            test_metadata,
            expected_job_count,
            test_listing: Arc::new(Mutex::new(Some(test_listing))),
            list_action,
            collector_options,
        })
    }
}

/// Enqueues test cases as jobs in the given client from the given artifact
///
/// This object is like an iterator, it maintains a position in the test listing and enqueues the
/// next thing when asked.
///
/// This object is stored inside `JobQueuing` and is used to keep track of which artifact it is
/// currently enqueuing from.
struct ArtifactQueuing<'a, MainAppDepsT: MainAppDeps> {
    log: slog::Logger,
    queuing_state: &'a JobQueuingState<MainAppDepsT::TestCollector>,
    deps: &'a MainAppDepsT,
    ui: UiSender,
    artifact: ArtifactM<MainAppDepsT>,
    ignored_cases: HashSet<String>,
    package_name: String,
    cases: CaseIter<CaseMetadataM<MainAppDepsT>>,
    timeout_override: Option<Option<Timeout>>,
    generated_artifacts: Option<GeneratedArtifacts>,
}

#[derive(Default)]
struct TestListingResult<CaseMetadataT> {
    cases: Vec<(String, CaseMetadataT)>,
    ignored_cases: HashSet<String>,
}

fn list_test_cases<TestCollectorT: CollectTests>(
    log: slog::Logger,
    queuing_state: &JobQueuingState<TestCollectorT>,
    ui: &UiSender,
    artifact: &TestCollectorT::Artifact,
    package_name: &str,
) -> Result<TestListingResult<TestCollectorT::CaseMetadata>> {
    ui.update_enqueue_status(format!("getting test list for {package_name}"));

    slog::debug!(log, "listing ignored tests"; "artifact" => ?artifact);
    let ignored_cases: HashSet<_> = artifact.list_ignored_tests()?.into_iter().collect();

    slog::debug!(log, "listing tests"; "artifact" => ?artifact);
    let mut cases = artifact.list_tests()?;

    let artifact_key = artifact.to_key();
    let mut listing = queuing_state.test_listing.lock().unwrap();
    listing.as_mut().unwrap().update_artifact_cases(
        package_name,
        artifact_key.clone(),
        cases.clone(),
    );

    cases.retain(|(c, cd)| {
        queuing_state
            .filter
            .filter(package_name, Some(&artifact_key), Some((c.as_str(), cd)))
            .expect("should have case")
    });
    Ok(TestListingResult {
        cases,
        ignored_cases,
    })
}

impl<'a, MainAppDepsT> ArtifactQueuing<'a, MainAppDepsT>
where
    MainAppDepsT: MainAppDeps,
{
    #[allow(clippy::too_many_arguments)]
    fn new(
        log: slog::Logger,
        queuing_state: &'a JobQueuingState<MainAppDepsT::TestCollector>,
        deps: &'a MainAppDepsT,
        ui: UiSender,
        artifact: ArtifactM<MainAppDepsT>,
        package_name: String,
        timeout_override: Option<Option<Timeout>>,
    ) -> Result<Self> {
        let listing = list_test_cases(log.clone(), queuing_state, &ui, &artifact, &package_name)?;

        ui.update_enqueue_status(format!("generating artifacts for {package_name}"));
        slog::debug!(
            log,
            "generating artifacts";
            "package_name" => &package_name,
            "artifact" => ?artifact);

        Ok(Self {
            log,
            queuing_state,
            deps,
            ui,
            artifact,
            ignored_cases: listing.ignored_cases,
            package_name,
            cases: listing.cases.into_iter(),
            timeout_override,
            generated_artifacts: None,
        })
    }

    fn calculate_job_layers(
        &mut self,
        test_metadata: &TestMetadata,
    ) -> Result<Vec<(Sha256Digest, ArtifactType)>> {
        test_metadata
            .layers
            .iter()
            .map(|layer| {
                slog::debug!(self.log, "adding layer"; "layer" => ?layer);
                self.deps.client().add_layer(layer.clone())
            })
            .collect::<Result<Vec<_>>>()
    }

    fn generate_artifacts(&mut self) -> Result<GeneratedArtifacts> {
        if let Some(generated_artifacts) = &self.generated_artifacts {
            return Ok(generated_artifacts.clone());
        }
        let generated_artifacts = artifacts::add_generated_artifacts(
            self.deps.client(),
            self.artifact.path(),
            self.log.clone(),
        )?;
        self.generated_artifacts = Some(generated_artifacts.clone());
        Ok(generated_artifacts)
    }

    fn queue_job_from_case(
        &mut self,
        case_name: &str,
        case_metadata: &CaseMetadataM<MainAppDepsT>,
    ) -> Result<EnqueueResult> {
        let case_str = self
            .artifact
            .format_case(&self.package_name, case_name, case_metadata);
        self.ui
            .update_enqueue_status(format!("processing {case_str}"));
        slog::debug!(self.log, "enqueuing test case"; "case" => &case_str);

        if self.queuing_state.list_action.is_some() {
            self.ui.list(case_str);
            return Ok(EnqueueResult::Listed);
        }

        let test_metadata = self
            .queuing_state
            .test_metadata
            .get_metadata_for_test_with_env(
                &self.package_name,
                &self.artifact.to_key(),
                (case_name, case_metadata),
            )?;
        self.ui
            .update_enqueue_status(format!("calculating layers for {case_str}"));
        slog::debug!(&self.log, "calculating job layers"; "case" => &case_str);
        let mut layers = self.calculate_job_layers(&test_metadata)?;

        match self
            .deps
            .test_collector()
            .get_test_layers(&test_metadata, &self.ui)?
        {
            TestLayers::GenerateForBinary => {
                let dep = self.generate_artifacts()?;
                layers.push((dep.binary, ArtifactType::Manifest));
                if test_metadata.include_shared_libraries() {
                    layers.push((dep.deps, ArtifactType::Manifest));
                }
            }
            TestLayers::Provided(layer_specs) => {
                for layer_spec in layer_specs {
                    layers.push(self.deps.client().add_layer(layer_spec)?);
                }
            }
        }

        // N.B. Must do this before we enqueue the job, but after we know we can't fail
        let count = self
            .queuing_state
            .jobs_queued
            .fetch_add(1, Ordering::AcqRel);
        self.ui.update_length(std::cmp::max(
            self.queuing_state.expected_job_count,
            count + 1,
        ));
        self.ui.job_enqueued(case_str.clone());
        self.queuing_state.tracker.add_outstanding();

        let visitor = JobStatusVisitor::new(
            self.queuing_state.tracker.clone(),
            self.queuing_state.test_listing.clone(),
            self.package_name.clone(),
            self.artifact.to_key(),
            case_name.to_owned(),
            case_str.clone(),
            self.ui.clone(),
            MainAppDepsT::TestCollector::remove_fixture_output
                as fn(&str, Vec<String>) -> Vec<String>,
        );

        if self.ignored_cases.contains(case_name) {
            visitor.job_ignored();
            return Ok(EnqueueResult::Ignored);
        }

        let estimated_duration = self
            .queuing_state
            .test_listing
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .get_timing(&self.package_name, &self.artifact.to_key(), case_name);

        self.ui
            .update_enqueue_status(format!("submitting job for {case_str}"));
        slog::debug!(&self.log, "submitting job"; "case" => &case_str);
        let (program, arguments) = self.artifact.build_command(case_name, case_metadata);
        self.deps.client().add_job(
            JobSpec {
                program,
                arguments,
                image: test_metadata.image,
                environment: test_metadata.environment,
                layers,
                mounts: test_metadata.mounts,
                network: test_metadata.network,
                root_overlay: if test_metadata.enable_writable_file_system {
                    JobRootOverlay::Tmp
                } else {
                    JobRootOverlay::None
                },
                working_directory: test_metadata.working_directory,
                user: test_metadata.user,
                group: test_metadata.group,
                timeout: self.timeout_override.unwrap_or(test_metadata.timeout),
                estimated_duration,
                allocate_tty: None,
            },
            move |res| visitor.job_finished(res),
        )?;

        Ok(EnqueueResult::Enqueued {
            package_name: self.package_name.clone(),
            case: case_name.into(),
        })
    }

    /// Attempt to enqueue the next test as a job in the client
    ///
    /// Returns an `EnqueueResult` describing what happened. Meant to be called until it returns
    /// `EnqueueResult::Done`
    fn enqueue_one(&mut self) -> Result<EnqueueResult> {
        let Some((case_name, case_metadata)) = self.cases.next() else {
            return Ok(EnqueueResult::Done);
        };
        self.queue_job_from_case(&case_name, &case_metadata)
    }
}

/// Enqueues tests as jobs using the given deps.
///
/// This object is like an iterator, it maintains a position in the test listing and enqueues the
/// next thing when asked.
struct JobQueuing<'a, MainAppDepsT: MainAppDeps> {
    log: slog::Logger,
    queuing_state: &'a JobQueuingState<MainAppDepsT::TestCollector>,
    deps: &'a MainAppDepsT,
    ui: UiSender,
    wait_handle: Option<BuildHandleM<MainAppDepsT>>,
    package_match: bool,
    artifacts: Option<ArtifactStreamM<MainAppDepsT>>,
    artifact_queuing: Option<ArtifactQueuing<'a, MainAppDepsT>>,
    timeout_override: Option<Option<Timeout>>,
}

impl<'a, MainAppDepsT> JobQueuing<'a, MainAppDepsT>
where
    MainAppDepsT: MainAppDeps,
{
    fn new(
        log: slog::Logger,
        queuing_state: &'a JobQueuingState<MainAppDepsT::TestCollector>,
        deps: &'a MainAppDepsT,
        ui: UiSender,
        timeout_override: Option<Option<Timeout>>,
    ) -> Result<Self> {
        ui.update_enqueue_status(MainAppDepsT::TestCollector::ENQUEUE_MESSAGE);

        let building_tests = !queuing_state.packages.is_empty()
            && matches!(
                queuing_state.list_action,
                None | Some(ListAction::ListTests)
            );

        let (wait_handle, artifacts) = building_tests
            .then(|| {
                deps.test_collector().start(
                    queuing_state.stderr_color,
                    &queuing_state.collector_options,
                    queuing_state.packages.values().collect(),
                    &ui,
                )
            })
            .transpose()?
            .unzip();

        Ok(Self {
            log,
            queuing_state,
            deps,
            ui,
            package_match: false,
            artifacts,
            artifact_queuing: None,
            wait_handle,
            timeout_override,
        })
    }

    fn start_queuing_from_artifact(&mut self) -> Result<bool> {
        self.ui
            .update_enqueue_status(MainAppDepsT::TestCollector::ENQUEUE_MESSAGE);

        slog::debug!(self.log, "getting artifacts");
        let Some(ref mut artifacts) = self.artifacts else {
            return Ok(false);
        };
        let Some(artifact) = artifacts.next() else {
            return Ok(false);
        };
        let artifact = artifact?;

        slog::debug!(self.log, "got artifact"; "artifact" => ?artifact);
        let package_name = self
            .queuing_state
            .packages
            .get(&artifact.package())
            .expect("artifact for unknown package")
            .name();

        self.artifact_queuing = Some(ArtifactQueuing::new(
            self.log.clone(),
            self.queuing_state,
            self.deps,
            self.ui.clone(),
            artifact,
            package_name.into(),
            self.timeout_override,
        )?);

        Ok(true)
    }

    /// Meant to be called when the user has enqueued all the jobs they want. Checks for deferred
    /// errors from collecting tests or otherwise
    fn finish(&mut self) -> Result<()> {
        slog::debug!(self.log, "checking for collection errors");
        if let Some(wh) = self.wait_handle.take() {
            wh.wait()?;
        }
        Ok(())
    }

    /// Attempt to enqueue the next test as a job in the client
    ///
    /// Returns an `EnqueueResult` describing what happened. Meant to be called it returns
    /// `EnqueueResult::Done`
    fn enqueue_one(&mut self) -> Result<EnqueueResult> {
        slog::debug!(self.log, "enqueuing a job");

        if self.artifact_queuing.is_none() && !self.start_queuing_from_artifact()? {
            self.finish()?;
            return Ok(EnqueueResult::Done);
        }
        self.package_match = true;

        let res = self.artifact_queuing.as_mut().unwrap().enqueue_one()?;
        if res.is_done() {
            self.artifact_queuing = None;
            return self.enqueue_one();
        }

        Ok(res)
    }
}

/// This is where cached data goes. If there is build output it is also here.
pub struct BuildDir;

/// A collection of objects that are used to run the MainApp. This is useful as a separate object
/// since it can contain things which live longer than scoped threads and thus shared among them.
pub struct MainAppState<MainAppDepsT: MainAppDeps> {
    deps: MainAppDepsT,
    queuing_state: JobQueuingState<MainAppDepsT::TestCollector>,
    test_listing_store: TestListingStore<ArtifactKeyM<MainAppDepsT>, CaseMetadataM<MainAppDepsT>>,
    logging_output: LoggingOutput,
    log: slog::Logger,
}

impl<MainAppDepsT: MainAppDeps> MainAppState<MainAppDepsT> {
    /// Creates a new `MainAppState`
    ///
    /// `bg_proc`: handle to background client process
    /// `include_filter`: tests which match any of the patterns in this filter are run
    /// `exclude_filter`: tests which match any of the patterns in this filter are not run
    /// `list_action`: if some, tests aren't run, instead tests or other things are listed
    /// `stderr_color`: should terminal color codes be written to `stderr` or not
    /// `project_dir`: the path to the root of the project
    /// `packages`: a listing of all the packages
    /// `broker_addr`: the network address of the broker which we connect to
    /// `client_driver`: an object which drives the background work of the `Client`
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        deps: MainAppDepsT,
        include_filter: Vec<String>,
        exclude_filter: Vec<String>,
        list_action: Option<ListAction>,
        stderr_color: bool,
        project_dir: impl AsRef<Root<ProjectDir>>,
        packages: &[PackageM<MainAppDepsT>],
        state_dir: impl AsRef<Root<StateDir>>,
        collector_options: CollectOptionsM<MainAppDepsT>,
        logging_output: LoggingOutput,
        log: slog::Logger,
    ) -> Result<Self> {
        slog::debug!(
            log, "creating app state";
            "include_filter" => ?include_filter,
            "exclude_filter" => ?exclude_filter,
            "list_action" => ?list_action,
        );

        let mut test_metadata =
            AllMetadata::load(log.clone(), project_dir, MainAppDepsT::MAELSTROM_TEST_TOML)?;
        let test_listing_store = TestListingStore::new(Fs::new(), &state_dir);
        let mut test_listing = test_listing_store.load()?;
        test_listing
            .retain_packages_and_artifacts(packages.iter().map(|p| (p.name(), p.artifacts())));

        let filter = TestFilterM::<MainAppDepsT>::compile(&include_filter, &exclude_filter)?;
        let selected_packages: BTreeMap<_, _> = packages
            .iter()
            .filter(|p| filter.filter(p.name(), None, None).unwrap_or(true))
            .map(|p| (p.id(), p.clone()))
            .collect();

        slog::debug!(
            log, "filtered packages";
            "selected_packages" => ?Vec::from_iter(selected_packages.keys()),
        );

        let vars = deps.get_template_vars(&collector_options)?;
        test_metadata.replace_template_vars(&vars)?;

        Ok(Self {
            deps,
            queuing_state: JobQueuingState::new(
                selected_packages,
                filter,
                stderr_color,
                test_metadata,
                test_listing,
                list_action,
                collector_options,
            )?,
            test_listing_store,
            logging_output,
            log,
        })
    }
}

/// The `MainApp` enqueues tests as jobs. With each attempted job enqueued this object is returned
/// and describes what happened.
pub enum EnqueueResult {
    /// A job successfully enqueued with the following information
    Enqueued { package_name: String, case: String },
    /// No job was enqueued, instead the test that would have been enqueued has been ignored
    /// because it has been marked as `#[ignored]`
    Ignored,
    /// No job was enqueued, we have run out of tests to run
    Done,
    /// No job was enqueued, we listed the test case instead
    Listed,
}

impl EnqueueResult {
    /// Is this `EnqueueResult` the `Done` variant
    pub fn is_done(&self) -> bool {
        matches!(self, Self::Done)
    }

    /// Is this `EnqueueResult` the `Ignored` variant
    pub fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

struct MainApp<'state, IntrospectDriverT, MainAppDepsT: MainAppDeps> {
    state: &'state MainAppState<MainAppDepsT>,
    queuing: JobQueuing<'state, MainAppDepsT>,
    introspect_driver: IntrospectDriverT,
    ui: UiSender,
}

impl<'state, 'scope, IntrospectDriverT, MainAppDepsT>
    MainApp<'state, IntrospectDriverT, MainAppDepsT>
where
    IntrospectDriverT: IntrospectDriver<'scope>,
    MainAppDepsT: MainAppDeps,
{
    pub fn new(
        state: &'state MainAppState<MainAppDepsT>,
        ui: UiSender,
        mut introspect_driver: IntrospectDriverT,
        timeout_override: Option<Option<Timeout>>,
    ) -> Result<Self>
    where
        'state: 'scope,
    {
        introspect_driver.drive(state.deps.client(), ui.clone());
        ui.update_length(state.queuing_state.expected_job_count);

        state
            .logging_output
            .update(UiSenderWriteAdapter::new(ui.clone()));
        slog::debug!(state.log, "main app created");

        let queuing = JobQueuing::new(
            state.log.clone(),
            &state.queuing_state,
            &state.deps,
            ui.clone(),
            timeout_override,
        )?;
        Ok(Self {
            state,
            queuing,
            introspect_driver,
            ui,
        })
    }

    /// Enqueue one test as a job on the `Client`. This is meant to be called repeatedly until
    /// `EnqueueResult::Done` is returned, or an error is encountered.
    pub fn enqueue_one(&mut self) -> Result<EnqueueResult> {
        self.queuing.enqueue_one()
    }

    /// Indicates that we have finished enqueuing jobs and starts tearing things down
    fn drain(&mut self) -> Result<()> {
        slog::debug!(self.queuing.log, "draining");
        self.ui
            .update_length(self.state.queuing_state.jobs_queued.load(Ordering::Acquire));
        self.ui.done_queuing_jobs();
        Ok(())
    }

    /// Waits for all outstanding jobs to finish, displays a summary, and obtains an `ExitCode`
    fn finish(&mut self) -> Result<ExitCode> {
        slog::debug!(self.queuing.log, "waiting for outstanding jobs");
        self.state.queuing_state.tracker.wait_for_outstanding();
        self.introspect_driver.stop()?;

        let summary = self.state.queuing_state.tracker.ui_summary();
        self.ui.finished(summary)?;

        self.state.test_listing_store.save(
            self.state
                .queuing_state
                .test_listing
                .lock()
                .unwrap()
                .take()
                .unwrap(),
        )?;

        Ok(self.state.queuing_state.tracker.exit_code())
    }
}

#[derive(Default)]
struct LoggingOutputInner {
    ui: Option<Box<dyn io::Write + Send + Sync + 'static>>,
}

#[derive(Clone, Default)]
pub struct LoggingOutput {
    inner: Arc<Mutex<LoggingOutputInner>>,
}

impl LoggingOutput {
    fn update(&self, ui: impl io::Write + Send + Sync + 'static) {
        let mut inner = self.inner.lock().unwrap();
        inner.ui = Some(Box::new(ui));
    }
}

impl io::Write for LoggingOutput {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(ui) = &mut inner.ui {
            ui.write(buf)
        } else {
            io::stdout().write(buf)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(ui) = &mut inner.ui {
            ui.flush()
        } else {
            io::stdout().flush()
        }
    }
}

pub enum Logger {
    DefaultLogger(LogLevel),
    GivenLogger(slog::Logger),
}

impl Logger {
    pub fn build(&self, out: LoggingOutput) -> slog::Logger {
        match self {
            Self::DefaultLogger(level) => {
                let decorator = slog_term::PlainDecorator::new(out);
                let drain = slog_term::FullFormat::new(decorator).build().fuse();
                let drain = slog_async::Async::new(drain).build().fuse();
                let drain = slog::LevelFilter::new(drain, level.as_slog_level()).fuse();
                slog::Logger::root(drain, slog::o!())
            }
            Self::GivenLogger(logger) => logger.clone(),
        }
    }
}

pub fn run_app_with_ui_multithreaded<MainAppDepsT>(
    state: MainAppState<MainAppDepsT>,
    timeout_override: Option<Option<Timeout>>,
    mut ui: impl Ui,
) -> Result<ExitCode>
where
    MainAppDepsT: MainAppDeps,
{
    let (ui_send, ui_recv) = std::sync::mpsc::channel();
    let ui_sender = UiSender::new(ui_send);
    let ui_handle = std::thread::spawn(move || ui.run(ui_recv));

    let exit_code_res = std::thread::scope(|scope| {
        let mut app = MainApp::new(
            &state,
            ui_sender.clone(),
            DefaultIntrospectDriver::new(scope),
            timeout_override,
        )?;
        while !app.enqueue_one()?.is_done() {}
        app.drain()?;
        app.finish()
    });
    drop(state);

    ui_sender.shutdown();
    let ui_res = ui_handle.join().unwrap();

    let exit_code = exit_code_res?;
    ui_res?;

    Ok(exit_code)
}

#[allow(clippy::too_many_arguments)]
pub fn main<
    ConfigT,
    ExtraCommandLineOptionsT,
    ArgsT,
    ArgsIntoIterT,
    IsListFn,
    GetProjectDirFn,
    MainFn,
>(
    command: Command,
    base_directories_prefix: &'static str,
    env_var_prefix: &'static str,
    args: ArgsIntoIterT,
    is_list: IsListFn,
    get_project_dir: GetProjectDirFn,
    test_toml_name: &str,
    test_toml_specific_contents: &str,
    main: MainFn,
) -> Result<ExitCode>
where
    ConfigT: Config + Debug + AsRef<config::Config>,
    ExtraCommandLineOptionsT: Args + AsRef<config::ExtraCommandLineOptions>,
    ArgsIntoIterT: IntoIterator<Item = ArgsT>,
    ArgsT: Into<OsString> + Clone,
    IsListFn: FnOnce(&ExtraCommandLineOptionsT) -> bool,
    GetProjectDirFn: FnOnce(&ConfigT) -> Result<Utf8PathBuf>,
    MainFn: FnOnce(
        ConfigT,
        ExtraCommandLineOptionsT,
        ClientBgProcess,
        Logger,
        bool,
        Box<dyn Ui>,
    ) -> Result<ExitCode>,
{
    let (config, extra_options): (ConfigT, ExtraCommandLineOptionsT) =
        maelstrom_util::config::new_config_with_extra_from_args(
            command,
            base_directories_prefix,
            env_var_prefix,
            args,
        )?;

    let config_parent = config.as_ref();

    let bg_proc = ClientBgProcess::new_from_fork(config_parent.log_level)?;
    let logger = Logger::DefaultLogger(config_parent.log_level);

    let stderr_is_tty = io::stderr().is_terminal();
    let stdout_is_tty = io::stdout().is_terminal();

    let ui = ui::factory(
        config_parent.ui,
        is_list(&extra_options),
        stdout_is_tty,
        config_parent.quiet,
    );

    if extra_options.as_ref().client_bg_proc {
        alternative_mains::client_bg_proc()
    } else if extra_options.as_ref().init {
        alternative_mains::init(
            &get_project_dir(&config)?,
            test_toml_name,
            test_toml_specific_contents,
        )
    } else {
        main(config, extra_options, bg_proc, logger, stderr_is_tty, ui)
    }
}
