use crate::abs_runner::{AbsRunOutput, PgoStatus};
use crate::app_settings::ThemePref;
use crate::config::ConfigDocument;
use crate::list_editors::PackageListField;
use crate::log_save::{LogSaveFormat, LogSaveTarget};
use crate::terminal_themes::TerminalTheme;
use iced::widget::text_editor;
use iced::{Point, Size};

#[derive(Debug, Clone, Copy)]
pub struct WindowCloseSnapshot {
    pub fullscreen: bool,
    pub maximized: bool,
    pub size: Size,
    pub position: Option<Point>,
}

/// Overlay showing the live AUR PKGBUILD for one package.
#[derive(Debug, Clone)]
pub struct PkgbuildPreview {
    pub name: String,
    pub version: Option<String>,
    pub text: Option<String>,
    pub error: Option<String>,
    pub show_delta: bool,
    /// Unified diff vs last known PKGBUILD. `None` if there is no previous copy.
    pub delta: Option<String>,
}

impl PkgbuildPreview {
    pub fn loading(name: String) -> Self {
        Self {
            name,
            version: None,
            text: None,
            error: None,
            show_delta: false,
            delta: None,
        }
    }

    pub fn copy_text(&self) -> Option<String> {
        if self.show_delta {
            match self.delta.as_deref() {
                Some(diff) if !diff.is_empty() => Some(diff.to_string()),
                _ => self.text.clone(),
            }
        } else {
            self.text.clone()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Page {
    Kernels,
    DefaultKernelConfig,
    KernelConfig,
    Packages,
    PackageConfig,
    SystemUpdate,
    AbsSettings,
    AppSettings,
    ConfigWizard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewportId {
    BuildLog,
    UpdateLog,
}

impl ViewportId {
    pub fn scroll_id(self) -> &'static str {
        match self {
            Self::BuildLog => "build-log",
            Self::UpdateLog => "update-log",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PackageListFilter {
    #[default]
    All,
    Kernels,
    PgoLto,
    Aur,
    Official,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PackageSortCol {
    #[default]
    Name,
    Source,
    Flags,
    Threads,
    Isolation,
}

#[derive(Debug, Clone)]
pub enum PackageConfirm {
    Remove(String),
    PurgeAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    GeneralPaths,
    BuildChroot,
    Ramdisk,
    HeldPackages,
    Repositories,
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
    PackagePurgeAll,
    PackageSort(PackageSortCol),
    PackageConfirmAccept,
    PackageConfirmCancel,
    OpenSystemUpdate,
    OpenAbsSettings,
    OpenConfigWizard,
    SettingsTabSelected(SettingsTab),
    KernelFilter(String),
    PackageFilter(String),
    KernelRowEnter(String),
    KernelRowExit(String),
    PackageRowEnter(String),
    PackageRowExit(String),
    PackageListFilter(PackageListFilter),
    FocusSearch,
    OpenAppSettings,
    Back,
    ReloadConfig,
    SaveConfig,
    SaveAppSettings,
    ConfigLoaded(Box<Result<ConfigDocument, String>>),
    ConfigSaved(Result<(), String>),
    AppSettingsSaved(Result<(), String>),
    AppThemeSelected(ThemePref),
    GuiLangSelected(Option<String>),
    AbsLangSelected(Option<String>),
    TerminalThemePreview(crate::app_settings::AppTheme, TerminalTheme),
    TerminalThemeApply,
    TerminalLinesLimitInput(String),
    TerminalLinesLimitDec,
    TerminalLinesLimitInc,
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
    SelfUpdateInstallPath(String),
    SelfUpdateUsePacman(Option<bool>),
    InstallAbsGui(Option<bool>),
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
    HeldNameChanged(usize, String),
    HeldVersionChanged(usize, String),
    HeldTriggersChanged(usize, String),
    HeldAdd,
    HeldRemove(usize),
    /// Fill empty trigger versions from `pacman -Q` for this held entry.
    HeldSnapshotTriggers(usize),
    HeldCheck,
    HeldCheckDone(Result<String, String>),
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
    SystemUpdateStart,
    SystemUpdateAbort,
    PendingUpdatesRefresh,
    PendingUpdatesLoaded(Result<crate::abs_runner::PendingUpdates, String>),
    /// Redraw the system-update fetch overlay (spinning gear).
    FetchOverlayTick,
    InstallRepoUpdates,
    InstallAur(String),
    LogFlush,
    PgoRunFinished(Result<AbsRunOutput, String>),
    PgoAbortFinished(Result<String, String>),
    LogClear,
    LogCopy,
    LogSave,
    LogSavePicked(Option<String>),
    LogSaveFinished(Result<(String, Option<String>), String>),
    LogSavePath(LogSaveTarget, String),
    LogSaveBrowse(LogSaveTarget),
    LogSaveFolderPicked(LogSaveTarget, Option<String>),
    LogSaveDontAsk(LogSaveTarget, bool),
    LogSaveFormat(LogSaveTarget, LogSaveFormat),
    ViewportScrolled(ViewportId, bool),
    ViewportAutoscroll(ViewportId, bool),
    AbsStdinChanged(String),
    AbsStdinSubmit,
    UnsavedSave,
    UnsavedDiscard,
    UnsavedCancel,
    // Window
    WindowResized(Size),
    WindowMoved(Point),
    WindowSizeCommitted {
        size: Size,
        fullscreen: bool,
        maximized: bool,
    },
    WindowPositionCommitted {
        point: Point,
        fullscreen: bool,
        maximized: bool,
    },
    WindowClampToMonitor {
        id: iced::window::Id,
        monitor: Option<Size>,
        size: Size,
        position: Option<Point>,
    },
    WindowCloseSnapshot(Option<WindowCloseSnapshot>),
    WindowCloseRequested,
    ExitAfterCleanup,
    // Live Telemetry
    SystemMetricsTick,
    // Config wizard (abs --config-wizard-form / check / apply)
    WizardFormLoaded(Result<crate::abs_runner::WizardForm, String>),
    WizardFieldChanged(String, serde_json::Value, bool),
    WizardCheckResult(u64, String, Result<(), String>),
    WizardStepChecked(Vec<(String, String)>),
    WizardBrowse(String, PathKind),
    WizardPathPicked(String, Option<String>),
    WizardUseSuggested(String),
    WizardListDraft(String, String),
    WizardListAdd(String),
    WizardRepoDraftName(String),
    WizardRepoDraftUrl(String),
    WizardRepoAdd(String),
    WizardNext,
    WizardBack,
    WizardApply,
    WizardApplyDone(Result<String, String>),
    WizardCancel,
    WizardTimer,
    /// Fetch the live AUR PKGBUILD for this package name.
    PreviewPkgbuild(String),
    PkgbuildLoaded {
        requested: String,
        result: Result<crate::abs_runner::AurPkgbuild, String>,
    },
    ClosePkgbuildPreview,
    CopyPkgbuild,
    TogglePkgbuildDelta,
}
