use crate::abs_runner::{AbsRunOutput, PgoStatus};
use crate::app_settings::AppTheme;
use crate::config::ConfigDocument;
use crate::list_editors::PackageListField;
use iced::widget::text_editor;
use iced::{Point, Size};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Kernels,
    DefaultKernelConfig,
    KernelConfig,
    Packages,
    PackageConfig,
    AbsSettings,
    AppSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    Folder,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathField {
    PackagesPath,
    ChrootPath,
    ReadyPath,
    ChrootMakepkgConf,
    RamdiskMountPoint,
    RamdiskSeedChroot,
    SelfUpdateInstallPath,
    PgoArchiveDir,
    PgoBenchmark,
    PgoBenchmarkWorkdir,
    PgoProfileScratchDir,
    PgoVmlinux,
    PgoStateFile,
}

#[derive(Debug, Clone, Copy)]
pub enum EditTarget {
    Default,
    Selected,
    /// Package selected on the Packages page (no kernel-defaults seeding).
    Package,
}

#[derive(Debug, Clone, Copy)]
pub enum KStr {
    Source,
    BuildEnv,
    Ramdisk,
    Alias,
    Compiler,
    UpstreamGithub,
    PreUpdateCommand,
    PostUpdateCommand,
    CustomLocalBuildCommand,
    CustomChrootBuildCommand,
    Cpusched,
    ProcessorOpt,
    LlvmLto,
    HzTicks,
    Tickrate,
    Preempt,
    Hugepage,
    ArchiveDir,
    Benchmark,
    BenchmarkWorkdir,
    BenchmarkPreset,
    ProfilingQuality,
    BuildUser,
    SysctlCommand,
    PgoPreset,
    ProfileScratchDir,
    PerfEventArgs,
    PerfExtraArgs,
    Vmlinux,
    AfdoTool,
    PropellerTool,
    AfdoProfileName,
    StateFile,
}

#[derive(Debug, Clone, Copy)]
pub enum KBool {
    PgoEnabled,
    PgoAutoRestart,
    PgoPerfDataOnRam,
    PgoVerifyBoot,
    CcHarder,
    LtoSuffix,
    GccSuffix,
    Kcfi,
}

/// Tri-state (unset / true / false) per-package options.
#[derive(Debug, Clone, Copy)]
pub enum KOptBool {
    Tests,
    UpstreamPrereleases,
}

#[derive(Debug, Clone, Copy)]
pub enum RamdiskLetter {
    Workdir,
    Chroot,
    Packages,
    Profiles,
}

#[derive(Debug, Clone)]
pub enum Message {
    OpenKernels,
    OpenDefaultConfig,
    OpenKernel(String),
    OpenPackages,
    OpenPackage(String),
    NewPackageNameChanged(String),
    PackageAdd,
    PackageRemove(String),
    OpenAbsSettings,
    OpenAppSettings,
    Back,
    ReloadConfig,
    SaveConfig,
    SaveAppSettings,
    ConfigLoaded(Box<Result<ConfigDocument, String>>),
    ConfigSaved(Result<(), String>),
    AppSettingsSaved(Result<(), String>),
    AppThemeSelected(AppTheme),
    // ABS settings
    PathPackages(String),
    PathChroot(String),
    PathReady(String),
    PathChrootMakepkg(String),
    BuildDefaultEnv(String),
    BuildDefaultCompiler(String),
    BuildConcurrentRepos(String),
    BuildConcurrentCompilations(String),
    BuildGlobalCpuThreadsMode(String),
    BuildGlobalCpuThreadsCap(String),
    BuildMaximumCpuThreadsCap(String),
    BuildDefaultCompilationThreads(String),
    BuildSystemUpdateFirst(bool),
    BuildIgnoreFailures(bool),
    BuildCompileFirstInstall(bool),
    BuildCleanInstallDefault(bool),
    BuildIgnoreAlreadyMade(bool),
    BuildFastAurRpc(bool),
    BuildCleanChrootAfter(bool),
    CheckForUpdateOnStartup(Option<bool>),
    AutoUpdateOnStartup(Option<bool>),
    SelfUpdateAtUpdates(Option<bool>),
    SelfUpdateRawUrl(String),
    SelfUpdateInstallPath(String),
    SelfUpdateUsePacman(Option<bool>),
    InstallTestingPhaseArchPackages(Option<bool>),
    PackageListEdited(PackageListField, text_editor::Action),
    UseSeparateSkipInstallAfter(bool),
    SysUpdateReposCmd(String),
    SysUpdateFullCmd(String),
    SysUpdateNoRefreshCmd(String),
    SysUpdateIgnoreFlag(String),
    // SysUpdateIgnorePackages handled via PackageListEdited
    RamdiskEnabled(bool),
    RamdiskWorkdir(bool),
    RamdiskChroot(bool),
    RamdiskPackages(bool),
    RamdiskSize(String),
    RamdiskMode(String),
    RamdiskMountPoint(String),
    RamdiskSeedChroot(String),
    RamdiskSyncOnExit(bool),
    RamdiskMinFreeRam(String),
    RamdiskWarnPackages(bool),
    RamdiskReclaimOnStartup(bool),
    RepoUrlChanged(String, String),
    RepoAdd,
    RepoRemove(String),
    CompilerCcChanged(String, String),
    CompilerCxxChanged(String, String),
    CompilerAdd,
    CompilerRemove(String),
    BrowsePath(PathField, PathKind),
    PathPicked(PathField, Option<String>),
    // Kernel editing
    SetKernelStr(EditTarget, KStr, String),
    SetKernelBool(EditTarget, KBool, bool),
    SetPackageOptBool(EditTarget, KOptBool, Option<bool>),
    PackageCompilationThreads(EditTarget, String),
    PackageCompileAlone(EditTarget, bool),
    PackageCompilationPriority(EditTarget, String),
    SetRamdiskTarget(EditTarget, RamdiskLetter, bool),
    CustomKernelChanged(String),
    // PGO
    RefreshPgoStatus,
    PgoStatusLoaded(Result<PgoStatus, String>),
    /// User picked a pipeline phase in the UI (does not run abs).
    PgoSelectStage(String),
    /// Clear saved state and run stage 1 (`--pgo-restart`).
    PgoRestartFromScratch,
    /// Run the selected phase (`--pgo-resume --pgo-stage … --pgo-once`).
    PgoStartFromPhase,
    /// Continue after a reboot wait gate (`--pgo-resume --pgo-once`).
    PgoContinueAfterReboot,
    PgoAbort,
    KernelBuildStart,
    PgoLogLine(String),
    PgoRunFinished(Result<AbsRunOutput, String>),
    PgoAbortFinished(Result<String, String>),
    LogClear,
    LogCopy,
    LogEdited(text_editor::Action),
    LogFollowTail,
    // Window
    WindowResized(Size),
    WindowMoved(Point),
    WindowCloseRequested,
    ExitAfterCleanup,
}
