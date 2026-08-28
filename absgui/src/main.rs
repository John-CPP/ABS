mod abs_runner;
mod app;
mod app_settings;
mod config;
mod dialog;
mod field_help;
mod list_editors;
mod log_save;
mod log_view;
mod messages;
mod metrics;
mod pkgbuild_diff;
mod ramdisk_size;
mod style;
mod system_theme;
mod terminal_themes;
mod views;
mod widgets;

fn main() -> iced::Result {
    apply_linux_wgpu_backend_default();
    app::run()
}

/// Prefer Vulkan on Linux when `WGPU_BACKEND` is unset so iced does not probe
/// OpenGL/EGL (Mesa DRI3 warnings on XWayland).
fn linux_wgpu_backend_default(existing: Option<&str>) -> Option<&'static str> {
    match existing {
        None | Some("") => Some("vulkan"),
        Some(_) => None,
    }
}

fn apply_linux_wgpu_backend_default() {
    #[cfg(target_os = "linux")]
    if let Some(backend) = linux_wgpu_backend_default(std::env::var("WGPU_BACKEND").ok().as_deref())
    {
        std::env::set_var("WGPU_BACKEND", backend);
    }
}

#[cfg(test)]
mod gpu_backend_tests {
    use super::linux_wgpu_backend_default;

    #[test]
    fn defaults_to_vulkan_when_unset() {
        assert_eq!(linux_wgpu_backend_default(None), Some("vulkan"));
    }

    #[test]
    fn defaults_to_vulkan_when_empty() {
        assert_eq!(linux_wgpu_backend_default(Some("")), Some("vulkan"));
    }

    #[test]
    fn respects_existing_backend() {
        assert_eq!(linux_wgpu_backend_default(Some("gl")), None);
        assert_eq!(linux_wgpu_backend_default(Some("vulkan")), None);
    }
}
