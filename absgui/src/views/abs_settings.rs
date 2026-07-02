use crate::config::ConfigDocument;
use crate::field_help;
use crate::list_editors::{ListEditors, PackageListField};
use crate::messages::Message;
use crate::app_settings::AppTheme;
use crate::widgets::{
    card_section, field_checkbox, field_number, field_path, field_pick, field_text,
    optional_bool_field, packages_list_editor, page_title, PathField, PathKind,
};
use iced::widget::{button, column, row, text, Space};
use iced::{Alignment, Element, Length};

const ENV_OPTS: &[&str] = &["local", "chroot"];
const RAMDISK_MODE_OPTS: &[&str] = &["0755", "0775", "0700"];

pub fn view<'a>(
    config: &'a ConfigDocument,
    editors: &'a ListEditors,
    app_theme: AppTheme,
) -> Element<'a, Message> {
    let paths = card_section(
        "Paths",
        app_theme,
        column![
            field_path(
                "packages_path",
                Some(field_help::PATH_PACKAGES),
                &config.paths.packages_path,
                "$XDG_CONFIG_HOME/.cache/abs/packages",
                PathField::PackagesPath,
                PathKind::Folder,
                app_theme,
                Message::PathPackages,
            ),
            field_path(
                "chroot_base_path",
                Some(field_help::PATH_CHROOT),
                &config.paths.chroot_base_path,
                "$XDG_CONFIG_HOME/.cache/abs/chroot",
                PathField::ChrootPath,
                PathKind::Folder,
                app_theme,
                Message::PathChroot,
            ),
            field_path(
                "ready_made_packages_path",
                Some(field_help::PATH_READY),
                &config.paths.ready_made_packages_path,
                "$XDG_CONFIG_HOME/.cache/abs/ready",
                PathField::ReadyPath,
                PathKind::Folder,
                app_theme,
                Message::PathReady,
            ),
            field_path(
                "chroot_makepkg_conf (optional file)",
                Some(field_help::PATH_CHROOT_MAKEPKG),
                config.paths.chroot_makepkg_conf.as_deref().unwrap_or(""),
                "~/.config/abs/makepkg.conf",
                PathField::ChrootMakepkgConf,
                PathKind::File,
                app_theme,
                Message::PathChrootMakepkg,
            ),
        ]
        .spacing(10),
    );

    let build = card_section(
        "Build",
        app_theme,
        column![
            field_pick(
                "default_environment",
                Some(field_help::DEFAULT_ENV),
                ENV_OPTS,
                &config.build.default_environment,
                app_theme,
                Message::BuildDefaultEnv,
            ),
            field_text(
                "default_compiler (optional)",
                Some(field_help::DEFAULT_COMPILER),
                config.build.default_compiler.as_deref().unwrap_or(""),
                "gcc14",
                app_theme,
                Message::BuildDefaultCompiler,
            ),
            row![
                field_number(
                    "concurrent_repos_downloads_limit",
                    Some(field_help::CONCURRENT_REPOS),
                    &config.build.concurrent_repos_downloads_limit.to_string(),
                    app_theme,
                    Message::BuildConcurrentRepos,
                ),
                field_number(
                    "concurrent_compilations_limit",
                    Some(field_help::CONCURRENT_COMPILATIONS),
                    &config.build.concurrent_compilations_limit.to_string(),
                    app_theme,
                    Message::BuildConcurrentCompilations,
                ),
            ]
            .spacing(12),
            field_checkbox(
                "system_update_first",
                Some(field_help::SYSTEM_UPDATE_FIRST),
                config.build.system_update_first,
                app_theme,
                Message::BuildSystemUpdateFirst,
            ),
            field_checkbox(
                "ignore_compilation_failures",
                Some(field_help::IGNORE_FAILURES),
                config.build.ignore_compilation_failures,
                app_theme,
                Message::BuildIgnoreFailures,
            ),
            field_checkbox(
                "compile_first_install_after",
                Some(field_help::COMPILE_FIRST_INSTALL),
                config.build.compile_first_install_after,
                app_theme,
                Message::BuildCompileFirstInstall,
            ),
            field_checkbox(
                "clean_install_by_default",
                Some(field_help::CLEAN_INSTALL_DEFAULT),
                config.build.clean_install_by_default,
                app_theme,
                Message::BuildCleanInstallDefault,
            ),
            field_checkbox(
                "fast_aur_rpc_update_checks",
                Some(field_help::FAST_AUR_RPC),
                config.build.fast_aur_rpc_update_checks,
                app_theme,
                Message::BuildFastAurRpc,
            ),
            field_checkbox(
                "clean_chroot_after_compilation",
                Some(field_help::CLEAN_CHROOT_AFTER),
                config.build.clean_chroot_after_compilation,
                app_theme,
                Message::BuildCleanChrootAfter,
            ),
        ]
        .spacing(10),
    );

    let self_update = card_section(
        "Self-update & startup",
        app_theme,
        column![
            optional_bool_field(
                "check_for_update_on_startup",
                Some(field_help::CHECK_UPDATE_STARTUP),
                config.check_for_update_on_startup,
                "true",
                app_theme,
                Message::CheckForUpdateOnStartup,
            ),
            optional_bool_field(
                "auto_update_on_startup",
                Some(field_help::AUTO_UPDATE_STARTUP),
                config.auto_update_on_startup,
                "false",
                app_theme,
                Message::AutoUpdateOnStartup,
            ),
            optional_bool_field(
                "self_update_at_updates",
                Some(field_help::SELF_UPDATE_AT_UPDATES),
                config.self_update_at_updates,
                "false",
                app_theme,
                Message::SelfUpdateAtUpdates,
            ),
            optional_bool_field(
                "install_testing_phase_archlinux_packages",
                Some(field_help::INSTALL_TESTING),
                config.install_testing_phase_archlinux_packages,
                "false",
                app_theme,
                Message::InstallTestingPhaseArchPackages,
            ),
            field_text(
                "self_update_github_url",
                Some(field_help::SELF_UPDATE_GITHUB),
                config.self_update_github_url.as_deref().unwrap_or(""),
                "https://github.com/John-CPP/ABS",
                app_theme,
                Message::SelfUpdateGithubUrl,
            ),
            field_text(
                "self_update_raw_url",
                Some(field_help::SELF_UPDATE_RAW),
                config.self_update_raw_url.as_deref().unwrap_or(""),
                "https://raw.githubusercontent.com/John-CPP/ABS/master/Cargo.toml",
                app_theme,
                Message::SelfUpdateRawUrl,
            ),
            optional_bool_field(
                "self_update_use_pacman",
                Some(field_help::SELF_UPDATE_USE_PACMAN),
                config.self_update_use_pacman,
                "auto",
                app_theme,
                Message::SelfUpdateUsePacman,
            ),
            field_path(
                "self_update_install_path",
                Some(field_help::SELF_UPDATE_INSTALL),
                config.self_update_install_path.as_deref().unwrap_or(""),
                "/usr/bin/abs",
                PathField::SelfUpdateInstallPath,
                PathKind::File,
                app_theme,
                Message::SelfUpdateInstallPath,
            ),
        ]
        .spacing(10),
    );

    let separate_skip_after = config.skip_install_packages_after_compilation.is_some();
    let package_lists = card_section(
        "Package lists",
        app_theme,
        column![
            packages_list_editor(
                "manual_update_packages",
                Some(field_help::MANUAL_UPDATE),
                editors.content(PackageListField::ManualUpdate),
                PackageListField::ManualUpdate,
                app_theme,
                true,
            ),
            packages_list_editor(
                "skip_install_packages",
                Some(field_help::SKIP_INSTALL),
                editors.content(PackageListField::SkipInstall),
                PackageListField::SkipInstall,
                app_theme,
                true,
            ),
            field_checkbox(
                "Use separate skip_install_packages_after_compilation",
                Some(field_help::USE_SEPARATE_SKIP_AFTER),
                separate_skip_after,
                app_theme,
                Message::UseSeparateSkipInstallAfter,
            ),
            packages_list_editor(
                "skip_install_packages_after_compilation",
                Some(field_help::SKIP_INSTALL_AFTER),
                editors.content(PackageListField::SkipInstallAfter),
                PackageListField::SkipInstallAfter,
                app_theme,
                separate_skip_after,
            ),
        ]
        .spacing(10),
    );

    let system_update = card_section(
        "System update",
        app_theme,
        column![
            field_text(
                "command_to_update_repositories",
                Some(field_help::SYS_REPOS_CMD),
                &config.system_update.command_to_update_repositories,
                "sudo pacman -Sy",
                app_theme,
                Message::SysUpdateReposCmd,
            ),
            field_text(
                "command_to_perform_system_update",
                Some(field_help::SYS_FULL_CMD),
                &config.system_update.command_to_perform_system_update,
                "sudo pacman -Syu",
                app_theme,
                Message::SysUpdateFullCmd,
            ),
            field_text(
                "command_to_perform_system_update_no_refresh",
                Some(field_help::SYS_NO_REFRESH_CMD),
                config
                    .system_update
                    .command_to_perform_system_update_no_refresh
                    .as_deref()
                    .unwrap_or(""),
                "sudo pacman -Su",
                app_theme,
                Message::SysUpdateNoRefreshCmd,
            ),
            field_text(
                "ignore_flag",
                Some(field_help::SYS_IGNORE_FLAG),
                &config.system_update.ignore_flag,
                "--ignore",
                app_theme,
                Message::SysUpdateIgnoreFlag,
            ),
            packages_list_editor(
                "ignore_packages",
                Some(field_help::SYS_IGNORE_PACKAGES),
                editors.content(PackageListField::SysUpdateIgnore),
                PackageListField::SysUpdateIgnore,
                app_theme,
                true,
            ),
        ]
        .spacing(10),
    );

    let ramdisk = card_section(
        "Ramdisk (tmpfs)",
        app_theme,
        column![
            field_checkbox(
                "enabled",
                Some(field_help::RAMDISK_ENABLED),
                config.ramdisk.enabled,
                app_theme,
                Message::RamdiskEnabled,
            ),
            field_path(
                "mount_point",
                Some(field_help::RAMDISK_MOUNT),
                &config.ramdisk.mount_point,
                "/run/abs-ram",
                PathField::RamdiskMountPoint,
                PathKind::Folder,
                app_theme,
                Message::RamdiskMountPoint,
            ),
            row![
                field_text(
                    "size",
                    Some(field_help::RAMDISK_SIZE),
                    &config.ramdisk.size,
                    "16G",
                    app_theme,
                    Message::RamdiskSize,
                ),
                field_pick(
                    "mode",
                    Some(field_help::RAMDISK_MODE),
                    RAMDISK_MODE_OPTS,
                    &config.ramdisk.mode,
                    app_theme,
                    Message::RamdiskMode,
                ),
            ]
            .spacing(12),
            field_checkbox(
                "build_workdir (w)",
                Some(field_help::RAMDISK_GLOBAL_W),
                config.ramdisk.build_workdir,
                app_theme,
                Message::RamdiskWorkdir,
            ),
            field_checkbox(
                "chroot (c)",
                Some(field_help::RAMDISK_GLOBAL_C),
                config.ramdisk.chroot,
                app_theme,
                Message::RamdiskChroot,
            ),
            field_checkbox(
                "packages (p)",
                Some(field_help::RAMDISK_GLOBAL_P),
                config.ramdisk.packages,
                app_theme,
                Message::RamdiskPackages,
            ),
            field_path(
                "seed_chroot_from (optional)",
                Some(field_help::RAMDISK_SEED),
                config.ramdisk.seed_chroot_from.as_deref().unwrap_or(""),
                "/path/to/chroot/seed",
                PathField::RamdiskSeedChroot,
                PathKind::Folder,
                app_theme,
                Message::RamdiskSeedChroot,
            ),
            field_checkbox(
                "sync_chroot_on_exit",
                Some(field_help::RAMDISK_SYNC),
                config.ramdisk.sync_chroot_on_exit,
                app_theme,
                Message::RamdiskSyncOnExit,
            ),
            row![
                field_number(
                    "min_free_ram_mb",
                    Some(field_help::RAMDISK_MIN_FREE),
                    &config.ramdisk.min_free_ram_mb.to_string(),
                    app_theme,
                    Message::RamdiskMinFreeRam,
                ),
                field_checkbox(
                    "warn_packages_ram",
                    Some(field_help::RAMDISK_WARN_PACKAGES),
                    config.ramdisk.warn_packages_ram,
                    app_theme,
                    Message::RamdiskWarnPackages,
                ),
                field_checkbox(
                    "reclaim_mount_on_startup",
                    Some(field_help::RAMDISK_RECLAIM),
                    config.ramdisk.reclaim_mount_on_startup,
                    app_theme,
                    Message::RamdiskReclaimOnStartup,
                ),
            ]
            .spacing(16)
            .align_y(Alignment::Center),
        ]
        .spacing(10),
    );

    let mut repo_rows = column![].spacing(8);
    let mut repo_names: Vec<_> = config.repositories.keys().cloned().collect();
    repo_names.sort();
    for name in repo_names {
        let url = config.repositories.get(&name).cloned().unwrap_or_default();
        repo_rows = repo_rows.push(row![
            text(name.clone()).size(14).width(Length::Fixed(100.0)),
            field_text("url", Some(field_help::REPO_URL), &url, "https://…", app_theme, {
                let n = name.clone();
                move |v| Message::RepoUrlChanged(n.clone(), v)
            }),
            button(text("Remove").size(13))
                .style(button::danger)
                .on_press(Message::RepoRemove(name)),
        ]
        .spacing(8)
        .align_y(Alignment::Center));
    }

    let repositories = card_section(
        "Repositories",
        app_theme,
        column![repo_rows, button(text("+ Add repository").size(13)).on_press(Message::RepoAdd)]
            .spacing(8),
    );

    let mut compiler_rows = column![].spacing(8);
    let mut compiler_names: Vec<_> = config.compilers.keys().cloned().collect();
    compiler_names.sort();
    for name in compiler_names {
        let cc = config.compilers.get(&name).map(|c| c.cc.clone()).unwrap_or_default();
        let cxx = config
            .compilers
            .get(&name)
            .map(|c| c.cxx.clone())
            .unwrap_or_default();
        compiler_rows = compiler_rows.push(row![
            text(name.clone()).size(14).width(Length::Fixed(80.0)),
            field_text("cc", Some(field_help::COMPILER_CC), &cc, "gcc-14", app_theme, {
                let n = name.clone();
                move |v| Message::CompilerCcChanged(n.clone(), v)
            }),
            field_text("cxx", Some(field_help::COMPILER_CXX), &cxx, "g++-14", app_theme, {
                let n = name.clone();
                move |v| Message::CompilerCxxChanged(n.clone(), v)
            }),
            button(text("Remove").size(13))
                .style(button::danger)
                .on_press(Message::CompilerRemove(name)),
        ]
        .spacing(8)
        .align_y(Alignment::Center));
    }

    let compilers = card_section(
        "Compilers",
        app_theme,
        column![
            compiler_rows,
            button(text("+ Add compiler").size(13)).on_press(Message::CompilerAdd),
        ]
        .spacing(8),
    );

    let actions = row![
        button(text("Reload").size(14))
            .style(button::secondary)
            .on_press(Message::ReloadConfig),
        button(text("Save").size(14))
            .style(button::primary)
            .on_press(Message::SaveConfig),
    ]
    .spacing(8);

    column![
        page_title("ABS settings", app_theme),
        text(format!("config_version = {}", config.config_version))
            .size(12)
            .color(crate::style::muted(app_theme)),
        paths,
        build,
        self_update,
        package_lists,
        system_update,
        ramdisk,
        repositories,
        compilers,
        actions,
        Space::new().height(Length::Fixed(8.0)),
    ]
    .spacing(16)
    .into()
}
