use crate::app_settings::AppTheme;
use crate::config::ConfigDocument;
use crate::field_help;
use crate::list_editors::{ListEditors, PackageListField};
use crate::messages::Message;
use crate::widgets::{
    card_section, field_checkbox, field_label_column, field_number, field_path, field_pick,
    field_text, optional_bool_field, packages_list_editor, PathField, PathKind,
};
use iced::widget::{button, column, row, text};
use iced::{Alignment, Element, Length};

const ENV_OPTS: &[&str] = &["local", "chroot"];
const CPU_THREADS_MODE_OPTS: &[&str] = &["strict", "flexible"];
const RAMDISK_MODE_OPTS: &[&str] = &["0755", "0775", "0700"];

pub fn view<'a>(
    config: &'a ConfigDocument,
    editors: &'a ListEditors,
    hold_check_report: Option<&'a str>,
    active_tab: crate::messages::SettingsTab,
    ram_total_bytes: u64,
    ram_used_bytes: u64,
    app_theme: AppTheme,
) -> Element<'a, Message> {
    let language = card_section(
        abs_i18n::t("gui.settings.language"),
        app_theme,
        column![abs_language_picker(config, app_theme)].spacing(10),
    );

    let paths = card_section(
        abs_i18n::t("gui.abs.paths"),
        app_theme,
        column![
            field_path(
                "packages_path",
                Some(field_help::path_packages()),
                &config.paths.packages_path,
                "$XDG_CACHE_HOME/abs/packages",
                PathField::PackagesPath,
                PathKind::Folder,
                app_theme,
                Message::PathPackages,
            ),
            field_path(
                "chroot_base_path",
                Some(field_help::path_chroot()),
                &config.paths.chroot_base_path,
                "$XDG_CACHE_HOME/abs/chroot",
                PathField::ChrootPath,
                PathKind::Folder,
                app_theme,
                Message::PathChroot,
            ),
            field_path(
                "ready_made_packages_path",
                Some(field_help::path_ready()),
                &config.paths.ready_made_packages_path,
                "$XDG_CACHE_HOME/abs/ready",
                PathField::ReadyPath,
                PathKind::Folder,
                app_theme,
                Message::PathReady,
            ),
            field_path(
                "chroot_makepkg_conf (optional file)",
                Some(field_help::path_chroot_makepkg()),
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
        abs_i18n::t("gui.abs.build"),
        app_theme,
        column![
            field_pick(
                "default_environment",
                Some(field_help::default_env()),
                ENV_OPTS,
                &config.build.default_environment,
                app_theme,
                Message::BuildDefaultEnv,
            ),
            field_text(
                "default_compiler (optional)",
                Some(field_help::default_compiler()),
                config.build.default_compiler.as_deref().unwrap_or(""),
                "gcc14",
                app_theme,
                Message::BuildDefaultCompiler,
            ),
            row![
                field_number(
                    "concurrent_repos_downloads_limit",
                    Some(field_help::concurrent_repos()),
                    &config.build.concurrent_repos_downloads_limit.to_string(),
                    app_theme,
                    Message::BuildConcurrentRepos,
                ),
                field_number(
                    "concurrent_compilations_limit",
                    Some(field_help::concurrent_compilations()),
                    &config.build.concurrent_compilations_limit.to_string(),
                    app_theme,
                    Message::BuildConcurrentCompilations,
                ),
            ]
            .spacing(12),
            row![
                field_pick(
                    "global_cpu_threads_mode",
                    Some(field_help::global_cpu_threads_mode()),
                    CPU_THREADS_MODE_OPTS,
                    &config.build.global_cpu_threads_mode,
                    app_theme,
                    Message::BuildGlobalCpuThreadsMode,
                ),
                field_number(
                    "global_cpu_threads_cap (optional)",
                    Some(field_help::global_cpu_threads_cap()),
                    &config
                        .build
                        .global_cpu_threads_cap
                        .map(|n| n.to_string())
                        .unwrap_or_default(),
                    app_theme,
                    Message::BuildGlobalCpuThreadsCap,
                ),
            ]
            .spacing(12),
            row![
                field_number(
                    "maximum_cpu_threads_cap (flexible)",
                    Some(field_help::maximum_cpu_threads_cap()),
                    &config
                        .build
                        .maximum_cpu_threads_cap
                        .map(|n| n.to_string())
                        .unwrap_or_default(),
                    app_theme,
                    Message::BuildMaximumCpuThreadsCap,
                ),
                field_number(
                    "default_compilation_threads (optional)",
                    Some(field_help::default_compilation_threads()),
                    &config
                        .build
                        .default_compilation_threads
                        .map(|n| n.to_string())
                        .unwrap_or_default(),
                    app_theme,
                    Message::BuildDefaultCompilationThreads,
                ),
            ]
            .spacing(12),
            field_checkbox(
                "system_update_first",
                Some(field_help::system_update_first()),
                config.build.system_update_first,
                app_theme,
                Message::BuildSystemUpdateFirst,
            ),
            field_checkbox(
                "ignore_compilation_failures",
                Some(field_help::ignore_failures()),
                config.build.ignore_compilation_failures,
                app_theme,
                Message::BuildIgnoreFailures,
            ),
            field_checkbox(
                "compile_first_install_after",
                Some(field_help::compile_first_install()),
                config.build.compile_first_install_after,
                app_theme,
                Message::BuildCompileFirstInstall,
            ),
            field_checkbox(
                "clean_install_by_default",
                Some(field_help::clean_install_default()),
                config.build.clean_install_by_default,
                app_theme,
                Message::BuildCleanInstallDefault,
            ),
            field_checkbox(
                "ignore_already_made_packages",
                Some(field_help::ignore_already_made()),
                config.build.ignore_already_made_packages,
                app_theme,
                Message::BuildIgnoreAlreadyMade,
            ),
            field_checkbox(
                "fast_aur_rpc_update_checks",
                Some(field_help::fast_aur_rpc()),
                config.build.fast_aur_rpc_update_checks,
                app_theme,
                Message::BuildFastAurRpc,
            ),
            field_checkbox(
                "clean_chroot_after_compilation",
                Some(field_help::clean_chroot_after()),
                config.build.clean_chroot_after_compilation,
                app_theme,
                Message::BuildCleanChrootAfter,
            ),
        ]
        .spacing(10),
    );

    let self_update = card_section(
        abs_i18n::t("gui.abs.self_update"),
        app_theme,
        column![
            optional_bool_field(
                "check_for_update_on_startup",
                Some(field_help::check_update_startup()),
                config.check_for_update_on_startup,
                "true",
                app_theme,
                Message::CheckForUpdateOnStartup,
            ),
            optional_bool_field(
                "auto_update_on_startup",
                Some(field_help::auto_update_startup()),
                config.auto_update_on_startup,
                "false",
                app_theme,
                Message::AutoUpdateOnStartup,
            ),
            optional_bool_field(
                "self_update_at_updates",
                Some(field_help::self_update_at_updates()),
                config.self_update_at_updates,
                "false",
                app_theme,
                Message::SelfUpdateAtUpdates,
            ),
            optional_bool_field(
                "install_testing_phase_archlinux_packages",
                Some(field_help::install_testing()),
                config.install_testing_phase_archlinux_packages,
                "false",
                app_theme,
                Message::InstallTestingPhaseArchPackages,
            ),
            optional_bool_field(
                "install_absgui",
                Some(field_help::install_absgui()),
                config.install_absgui,
                "true",
                app_theme,
                Message::InstallAbsGui,
            ),
            optional_bool_field(
                "self_update_use_pacman",
                Some(field_help::self_update_use_pacman()),
                config.self_update_use_pacman,
                "true",
                app_theme,
                Message::SelfUpdateUsePacman,
            ),
            field_path(
                "self_update_install_path",
                Some(field_help::self_update_install()),
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
        abs_i18n::t("gui.abs.package_lists"),
        app_theme,
        column![
            packages_list_editor(
                "manual_update_packages",
                Some(field_help::manual_update()),
                editors.content(PackageListField::ManualUpdate),
                PackageListField::ManualUpdate,
                app_theme,
                true,
            ),
            packages_list_editor(
                "skip_install_packages",
                Some(field_help::skip_install()),
                editors.content(PackageListField::SkipInstall),
                PackageListField::SkipInstall,
                app_theme,
                true,
            ),
            field_checkbox(
                abs_i18n::t("gui.abs.skip_after_separate"),
                Some(field_help::use_separate_skip_after()),
                separate_skip_after,
                app_theme,
                Message::UseSeparateSkipInstallAfter,
            ),
            packages_list_editor(
                "skip_install_packages_after_compilation",
                Some(field_help::skip_install_after()),
                editors.content(PackageListField::SkipInstallAfter),
                PackageListField::SkipInstallAfter,
                app_theme,
                separate_skip_after,
            ),
        ]
        .spacing(10),
    );

    let system_update = card_section(
        abs_i18n::t("gui.abs.system_update"),
        app_theme,
        column![
            field_text(
                "command_to_update_repositories",
                Some(field_help::sys_repos_cmd()),
                &config.system_update.command_to_update_repositories,
                "sudo pacman -Sy",
                app_theme,
                Message::SysUpdateReposCmd,
            ),
            field_text(
                "command_to_perform_system_update",
                Some(field_help::sys_full_cmd()),
                &config.system_update.command_to_perform_system_update,
                "sudo pacman -Syu",
                app_theme,
                Message::SysUpdateFullCmd,
            ),
            field_text(
                "command_to_perform_system_update_no_refresh",
                Some(field_help::sys_no_refresh_cmd()),
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
                Some(field_help::sys_ignore_flag()),
                &config.system_update.ignore_flag,
                "--ignore",
                app_theme,
                Message::SysUpdateIgnoreFlag,
            ),
            packages_list_editor(
                "ignore_packages",
                Some(field_help::sys_ignore_packages()),
                editors.content(PackageListField::SysUpdateIgnore),
                PackageListField::SysUpdateIgnore,
                app_theme,
                true,
            ),
        ]
        .spacing(10),
    );

    let ramdisk = card_section(
        abs_i18n::t("gui.abs.ramdisk"),
        app_theme,
        column![
            crate::widgets::ram_share_meter(
                abs_i18n::t("gui.abs.ramdisk_vs_ram"),
                &config.ramdisk.size,
                ram_total_bytes,
                ram_used_bytes,
                app_theme,
            ),
            field_checkbox(
                "enabled",
                Some(field_help::ramdisk_enabled()),
                config.ramdisk.enabled,
                app_theme,
                Message::RamdiskEnabled,
            ),
            field_path(
                "mount_point",
                Some(field_help::ramdisk_mount()),
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
                    Some(field_help::ramdisk_size()),
                    &config.ramdisk.size,
                    "16G",
                    app_theme,
                    Message::RamdiskSize,
                ),
                field_pick(
                    "mode",
                    Some(field_help::ramdisk_mode()),
                    RAMDISK_MODE_OPTS,
                    &config.ramdisk.mode,
                    app_theme,
                    Message::RamdiskMode,
                ),
            ]
            .spacing(12),
            field_checkbox(
                "build_workdir (w)",
                Some(field_help::ramdisk_global_w()),
                config.ramdisk.build_workdir,
                app_theme,
                Message::RamdiskWorkdir,
            ),
            field_checkbox(
                "chroot (c)",
                Some(field_help::ramdisk_global_c()),
                config.ramdisk.chroot,
                app_theme,
                Message::RamdiskChroot,
            ),
            field_checkbox(
                "packages (p)",
                Some(field_help::ramdisk_global_p()),
                config.ramdisk.packages,
                app_theme,
                Message::RamdiskPackages,
            ),
            field_path(
                "seed_chroot_from (optional)",
                Some(field_help::ramdisk_seed()),
                config.ramdisk.seed_chroot_from.as_deref().unwrap_or(""),
                "/path/to/chroot/seed",
                PathField::RamdiskSeedChroot,
                PathKind::Folder,
                app_theme,
                Message::RamdiskSeedChroot,
            ),
            field_checkbox(
                "sync_chroot_on_exit",
                Some(field_help::ramdisk_sync()),
                config.ramdisk.sync_chroot_on_exit,
                app_theme,
                Message::RamdiskSyncOnExit,
            ),
            row![
                field_number(
                    "min_free_ram_mb",
                    Some(field_help::ramdisk_min_free()),
                    &config.ramdisk.min_free_ram_mb.to_string(),
                    app_theme,
                    Message::RamdiskMinFreeRam,
                ),
                field_checkbox(
                    "warn_packages_ram",
                    Some(field_help::ramdisk_warn_packages()),
                    config.ramdisk.warn_packages_ram,
                    app_theme,
                    Message::RamdiskWarnPackages,
                ),
                field_checkbox(
                    "reclaim_mount_on_startup",
                    Some(field_help::ramdisk_reclaim()),
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
        repo_rows = repo_rows.push(
            row![
                text(name.clone()).size(14).width(Length::Fixed(100.0)),
                field_text(
                    "url",
                    Some(field_help::repo_url()),
                    &url,
                    "https://…",
                    app_theme,
                    {
                        let n = name.clone();
                        move |v| Message::RepoUrlChanged(n.clone(), v)
                    }
                ),
                button(text(abs_i18n::t("gui.common.remove")).size(13))
                    .style(crate::style::btn_danger(app_theme))
                    .on_press(Message::RepoRemove(name)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }

    let repositories = card_section(
        abs_i18n::t("gui.abs.repositories"),
        app_theme,
        column![
            repo_rows,
            button(text(abs_i18n::t("gui.abs.add_repo")).size(13))
                .style(crate::style::btn_secondary(app_theme))
                .on_press(Message::RepoAdd)
        ]
        .spacing(8),
    );

    let mut compiler_rows = column![].spacing(8);
    let mut compiler_names: Vec<_> = config.compilers.keys().cloned().collect();
    compiler_names.sort();
    for name in compiler_names {
        let cc = config
            .compilers
            .get(&name)
            .map(|c| c.cc.clone())
            .unwrap_or_default();
        let cxx = config
            .compilers
            .get(&name)
            .map(|c| c.cxx.clone())
            .unwrap_or_default();
        compiler_rows = compiler_rows.push(
            row![
                text(name.clone()).size(14).width(Length::Fixed(80.0)),
                field_text(
                    "cc",
                    Some(field_help::compiler_cc()),
                    &cc,
                    "gcc-14",
                    app_theme,
                    {
                        let n = name.clone();
                        move |v| Message::CompilerCcChanged(n.clone(), v)
                    }
                ),
                field_text(
                    "cxx",
                    Some(field_help::compiler_cxx()),
                    &cxx,
                    "g++-14",
                    app_theme,
                    {
                        let n = name.clone();
                        move |v| Message::CompilerCxxChanged(n.clone(), v)
                    }
                ),
                button(text(abs_i18n::t("gui.common.remove")).size(13))
                    .style(crate::style::btn_danger(app_theme))
                    .on_press(Message::CompilerRemove(name)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }

    let compilers = card_section(
        abs_i18n::t("gui.abs.compilers"),
        app_theme,
        column![
            compiler_rows,
            button(text(abs_i18n::t("gui.abs.add_compiler")).size(13))
                .style(crate::style::btn_secondary(app_theme))
                .on_press(Message::CompilerAdd),
        ]
        .spacing(8),
    );

    let mut held_rows = column![].spacing(12);
    if config.held_packages.is_empty() {
        held_rows = held_rows.push(
            text(abs_i18n::t("gui.abs.held_none"))
                .size(13)
                .color(crate::style::muted(app_theme)),
        );
    }
    for (idx, held) in config.held_packages.iter().enumerate() {
        let triggers = held.triggers_text();
        let name = held.name.clone();
        let has_pkg_config = config.packages.contains_key(&held.name) && !held.name.is_empty();
        let mut actions = row![
            button(text(abs_i18n::t("gui.abs.held_snapshot")).size(12))
                .style(crate::style::btn_secondary(app_theme))
                .on_press(Message::HeldSnapshotTriggers(idx)),
            button(text(abs_i18n::t("gui.common.remove")).size(12))
                .style(crate::style::btn_danger(app_theme))
                .on_press(Message::HeldRemove(idx)),
        ]
        .spacing(8);
        if has_pkg_config {
            actions = actions.push(
                button(text(abs_i18n::t("gui.abs.held_edit")).size(12))
                    .style(crate::style::btn_secondary(app_theme))
                    .on_press(Message::OpenPackage(name.clone())),
            );
        } else if !held.name.is_empty() {
            actions = actions.push(
                button(text(abs_i18n::t("gui.abs.held_add_pkg")).size(12))
                    .style(crate::style::btn_secondary(app_theme))
                    .on_press(Message::OpenPackage(name.clone())),
            );
        }

        held_rows = held_rows.push(
            column![
                row![
                    field_text(
                        "name",
                        Some(field_help::held_name()),
                        &held.name,
                        "libfoo",
                        app_theme,
                        move |v| Message::HeldNameChanged(idx, v),
                    ),
                    field_text(
                        "version",
                        Some(field_help::held_version()),
                        &held.version,
                        "1.2.3-1",
                        app_theme,
                        move |v| Message::HeldVersionChanged(idx, v),
                    ),
                ]
                .spacing(8),
                field_text(
                    "on_packages_updated (name[=ver], …)",
                    Some(field_help::held_triggers()),
                    &triggers,
                    "glibc=2.41-1, icu=76.1-1",
                    app_theme,
                    move |v| Message::HeldTriggersChanged(idx, v),
                ),
                actions,
            ]
            .spacing(8),
        );
    }

    let held_section_body = column![
        text(field_help::held_packages())
            .size(crate::style::TEXT_HELP)
            .color(crate::style::muted(app_theme)),
        held_rows,
        row![
            button(text(abs_i18n::t("gui.abs.held_add")).size(13))
                .style(crate::style::btn_primary(app_theme))
                .on_press(Message::HeldAdd),
            button(text(abs_i18n::t("gui.abs.held_check")).size(13))
                .style(crate::style::btn_secondary(app_theme))
                .on_press(Message::HeldCheck),
        ]
        .spacing(8),
        text(field_help::held_check())
            .size(crate::style::TEXT_HELP)
            .color(crate::style::muted(app_theme)),
        {
            if let Some(report) = hold_check_report {
                column![
                    text(abs_i18n::t("gui.abs.held_result")).size(crate::style::TEXT_LABEL),
                    text(report)
                        .size(crate::style::TEXT_BODY)
                        .font(iced::Font::MONOSPACE),
                ]
                .spacing(4)
            } else {
                column![]
            }
        },
    ]
    .spacing(10);

    let held_packages = card_section(abs_i18n::t("gui.abs.held"), app_theme, held_section_body);

    let tab_content: Element<'a, Message> = match active_tab {
        crate::messages::SettingsTab::GeneralPaths => row![
            column![language, paths].spacing(12).width(Length::Fill),
            column![self_update].spacing(12).width(Length::Fill),
        ]
        .spacing(12)
        .into(),
        crate::messages::SettingsTab::BuildChroot => column![
            build,
            row![
                column![package_lists].width(Length::Fill),
                column![system_update].width(Length::Fill),
            ]
            .spacing(12),
        ]
        .spacing(12)
        .into(),
        crate::messages::SettingsTab::Ramdisk => column![ramdisk].spacing(12).into(),
        crate::messages::SettingsTab::HeldPackages => column![held_packages].spacing(12).into(),
        crate::messages::SettingsTab::Repositories => row![
            column![repositories].width(Length::Fill),
            column![compilers].width(Length::Fill),
        ]
        .spacing(12)
        .into(),
    };

    let actions = row![
        button(text(abs_i18n::t("gui.wizard.open")).size(14))
            .style(crate::style::btn_secondary(app_theme))
            .on_press(Message::OpenConfigWizard),
        button(text(abs_i18n::t("gui.common.reload")).size(14))
            .style(crate::style::btn_secondary(app_theme))
            .on_press(Message::ReloadConfig),
        button(text(abs_i18n::t("gui.common.save_config")).size(14))
            .style(crate::style::btn_primary(app_theme))
            .on_press(Message::SaveConfig),
    ]
    .spacing(8);

    let tab_title = match active_tab {
        crate::messages::SettingsTab::GeneralPaths => abs_i18n::t("gui.abs.tab_general"),
        crate::messages::SettingsTab::BuildChroot => abs_i18n::t("gui.abs.tab_build"),
        crate::messages::SettingsTab::Ramdisk => abs_i18n::t("gui.abs.tab_ramdisk"),
        crate::messages::SettingsTab::HeldPackages => abs_i18n::t("gui.abs.tab_held"),
        crate::messages::SettingsTab::Repositories => abs_i18n::t("gui.abs.tab_repos"),
    };

    column![
        crate::widgets::breadcrumb_row(
            abs_i18n::t("gui.nav.abs_settings"),
            tab_title.to_string(),
            Some(crate::widgets::settings_tab_bar(active_tab, app_theme)),
            app_theme,
        ),
        text(format!("config_version = {}", config.config_version))
            .size(11)
            .color(crate::style::muted(app_theme)),
        tab_content,
        actions,
    ]
    .spacing(12)
    .into()
}

fn abs_language_picker(config: &ConfigDocument, theme: AppTheme) -> Element<'_, Message> {
    let system = abs_i18n::t("gui.settings.system");
    let mut opts: Vec<String> = vec![system.to_string()];
    opts.extend(abs_i18n::Lang::ALL.iter().map(|l| l.picker_label()));
    let selected = config
        .lang
        .as_deref()
        .and_then(abs_i18n::Lang::parse)
        .map(|l| l.picker_label())
        .unwrap_or_else(|| system.to_string());
    field_label_column(
        abs_i18n::t("gui.settings.language"),
        Some(abs_i18n::t("gui.settings.abs_language_help")),
        theme,
        crate::widgets::themed_pick_list(
            opts,
            Some(selected),
            |choice| {
                if choice == abs_i18n::t("gui.settings.system") {
                    Message::AbsLangSelected(None)
                } else {
                    Message::AbsLangSelected(
                        abs_i18n::Lang::ALL
                            .iter()
                            .find(|l| l.picker_label() == choice)
                            .map(|l| l.code().to_string()),
                    )
                }
            },
            theme,
            Length::Fill,
        ),
    )
}
