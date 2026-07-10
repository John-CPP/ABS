use crate::build;
use crate::build_env;
use crate::cli::Cli;
use crate::config::Config;
use crate::dep_graph;
use crate::git;
use crate::package_spec::PackageSpec;
use crate::pkgbuild;
use crate::ramdisk;
use crate::vlog;
use colored::Colorize;
use std::collections::{HashMap, HashSet};
use std::sync::{Condvar, Mutex};

#[derive(Debug, Clone)]
pub struct CompilationJob {
    pub base_name: String,
    pub spec: PackageSpec,
    pub threads: Option<usize>,
    pub compile_alone: bool,
    pub priority: usize,
}

#[derive(Debug, Clone)]
struct ActiveBuild {
    base_name: String,
    threads: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
struct PairingCaps {
    soft: Option<usize>,
    hard: Option<usize>,
}

#[derive(Clone)]
struct SchedulerState {
    pending: HashSet<String>,
    active: Vec<ActiveBuild>,
    finished: HashSet<String>,
    failed: HashSet<String>,
    exclusive_alone: bool,
}

/// Read-only data shared by all worker threads.
struct SchedulerShared<'a> {
    jobs: &'a HashMap<String, CompilationJob>,
    deps_map: &'a HashMap<String, HashSet<String>>,
    caps: PairingCaps,
    slot_limit: usize,
}

type JobGraph = (
    HashMap<String, CompilationJob>,
    HashMap<String, HashSet<String>>,
    HashMap<String, String>,
);

pub fn run_compilations(
    specs: Vec<PackageSpec>,
    cli: &Cli,
    config: &Config,
    defer_install: bool,
) -> HashSet<String> {
    let mut skipped_install = HashSet::new();
    if specs.is_empty() {
        return skipped_install;
    }
    crate::utils::request_exit_pause();

    let sorted_specs = match dep_graph::sort_packages_topologically(&specs, cli, config) {
        Ok(sorted) => sorted,
        Err(e) => {
            crate::ewarn!("Dependency sort failed: {}. Falling back to default order.", e);
            specs.clone()
        }
    };

    let slot_limit = config.build.concurrent_compilations_limit.max(1);

    if slot_limit <= 1 || !defer_install {
        for spec in &sorted_specs {
            crate::blog!("Processing package: {}", spec.name);
            let threads = build_env::resolve_package_threads(&spec.name, config, cli);
            if !build::process_package(spec, cli, config, defer_install, None, threads) {
                skipped_install.insert(spec.name.clone());
            }
        }
        return skipped_install;
    }

    vlog!(
        "Starting CPU-aware compilations (slots: {}, mode: {})...",
        slot_limit,
        config.build.global_cpu_threads_mode
    );

    let (jobs, deps_map, base_to_name) = build_job_graph(&sorted_specs, cli, config);

    let state = Mutex::new(SchedulerState {
        pending: jobs.keys().cloned().collect(),
        active: Vec::new(),
        finished: HashSet::new(),
        failed: HashSet::new(),
        exclusive_alone: false,
    });
    let cvar = Condvar::new();
    let shared = SchedulerShared {
        jobs: &jobs,
        deps_map: &deps_map,
        caps: pairing_caps(config),
        slot_limit,
    };

    std::thread::scope(|scope| {
        for worker_id in 0..slot_limit {
            let state = &state;
            let cvar = &cvar;
            let shared = &shared;
            scope.spawn(move || {
                worker_loop(worker_id, state, cvar, shared, cli, config, defer_install);
            });
        }
    });

    let guard = state.lock().unwrap();
    for base in &guard.failed {
        if let Some(name) = base_to_name.get(base) {
            skipped_install.insert(name.clone());
        }
    }
    skipped_install
}

fn build_job_graph(sorted_specs: &[PackageSpec], cli: &Cli, config: &Config) -> JobGraph {
    let mut jobs = HashMap::new();
    let mut base_to_name = HashMap::new();
    let mut pkg_dirs = Vec::new();

    for spec in sorted_specs {
        let (repo_name, repo_url_string, base) =
            build::resolve_pkg_repo_for_manual(&spec.name, cli, config);
        base_to_name.insert(base.clone(), spec.name.clone());
        let pkg_cfg = config.packages.get(&spec.name);
        let targets = ramdisk::resolve_ramdisk_targets(
            config,
            pkg_cfg,
            Some(spec),
            cli.ramdisk.as_deref(),
        )
        .unwrap_or_default();
        let pkg_dir = git::prepare_repo(
            &spec.name,
            &base,
            &repo_name,
            &repo_url_string,
            &ramdisk::download_packages_path(config, &targets),
            false,
            false,
            None,
        )
        .pkg_dir;
        pkg_dirs.push((base.clone(), pkg_dir));
        jobs.insert(
            base.clone(),
            CompilationJob {
                base_name: base,
                spec: spec.clone(),
                threads: build_env::resolve_package_threads(&spec.name, config, cli),
                compile_alone: pkg_cfg.is_some_and(|p| p.compile_alone),
                priority: pkg_cfg.map(|p| p.compilation_priority).unwrap_or(1),
            },
        );
    }

    let mut deps_map = HashMap::new();
    for (base, pkg_dir) in pkg_dirs {
        let all_deps = pkgbuild::parse_pkg_dependencies(pkg_dir.as_path());
        let mut filtered = HashSet::new();
        for dep in all_deps {
            if jobs.contains_key(&dep) && dep != base {
                filtered.insert(dep);
            }
        }
        deps_map.insert(base, filtered);
    }

    (jobs, deps_map, base_to_name)
}

fn worker_loop(
    worker_id: usize,
    state: &Mutex<SchedulerState>,
    cvar: &Condvar,
    shared: &SchedulerShared<'_>,
    cli: &Cli,
    config: &Config,
    defer_install: bool,
) {
    loop {
        let job = {
            let mut guard = state.lock().unwrap();
            loop {
                drain_blocked_pending(&mut guard, shared.deps_map);
                if guard.pending.is_empty() && guard.active.is_empty() {
                    return;
                }
                let picked = pick_next_job(&guard, shared)
                    .or_else(|| force_pick_if_stalled(&guard, shared));
                if let Some(base) = picked {
                    let job = shared.jobs.get(&base).unwrap().clone();
                    guard.pending.remove(&base);
                    guard.active.push(ActiveBuild {
                        base_name: base.clone(),
                        threads: job.threads,
                    });
                    if job.compile_alone {
                        guard.exclusive_alone = true;
                    }
                    break job;
                }
                guard = cvar.wait(guard).unwrap();
            }
        };

        crate::blog!(
            "Processing package [Worker {}]: {}",
            worker_id,
            job.spec.name
        );
        let chroot_copy = format!("abs-worker-{}", worker_id);
        let success = build::process_package(
            &job.spec,
            cli,
            config,
            defer_install,
            Some(&chroot_copy),
            job.threads,
        );

        {
            let mut guard = state.lock().unwrap();
            guard
                .active
                .retain(|a| a.base_name != job.base_name);
            if guard.exclusive_alone && job.compile_alone {
                guard.exclusive_alone = false;
            }
            if success {
                guard.finished.insert(job.base_name.clone());
            } else {
                guard.failed.insert(job.base_name.clone());
            }
            cvar.notify_all();
        }
    }
}

fn drain_blocked_pending(
    state: &mut SchedulerState,
    deps_map: &HashMap<String, HashSet<String>>,
) {
    let blocked: Vec<String> = state
        .pending
        .iter()
        .filter(|base| {
            deps_map
                .get(*base)
                .is_some_and(|deps| deps.iter().any(|d| state.failed.contains(d)))
        })
        .cloned()
        .collect();
    for base in blocked {
        vlog!("Parallel compile: Skipping {} because a dependency failed.", base);
        state.pending.remove(&base);
        state.failed.insert(base);
    }
}

/// Deadlock breaker: nothing is building, yet no pending job has satisfiable dependencies
/// (a dependency cycle survived the topological-sort fallback). Waiting would hang every
/// worker forever, so force-start the best pending job while ignoring its dependencies —
/// once it finishes, the rest of the cycle unblocks normally.
fn force_pick_if_stalled(state: &SchedulerState, shared: &SchedulerShared<'_>) -> Option<String> {
    if !state.active.is_empty() || state.pending.is_empty() {
        return None;
    }
    let mut candidates: Vec<CompilationJob> = state
        .pending
        .iter()
        .filter_map(|base| shared.jobs.get(base).cloned())
        .collect();
    if candidates.is_empty() {
        return None;
    }
    sort_ready(&mut candidates);
    let picked = &candidates[0];
    crate::ewarn!(
        "Parallel compile: {} has unsatisfiable dependencies (cycle?); starting it anyway.",
        picked.base_name
    );
    Some(picked.base_name.clone())
}

fn pick_next_job(state: &SchedulerState, shared: &SchedulerShared<'_>) -> Option<String> {
    if state.exclusive_alone {
        return None;
    }
    if state.active.len() >= shared.slot_limit {
        return None;
    }

    let mut ready = compute_ready(state, shared.deps_map, shared.jobs);
    if ready.is_empty() {
        return None;
    }
    sort_ready(&mut ready);

    if state.active.is_empty() {
        for job in &ready {
            if can_start(job, &state.active, shared.caps, shared.slot_limit, false) {
                return Some(job.base_name.clone());
            }
        }
        return None;
    }

    // The top-priority ready job wants exclusivity: stop admitting partners so the active
    // builds drain and it can start, instead of being starved by a stream of smaller jobs.
    if ready.first().is_some_and(|j| j.compile_alone) {
        return None;
    }

    if let Some(partner) = find_partner(&ready, &state.active, shared.caps)
        && can_start(partner, &state.active, shared.caps, shared.slot_limit, false)
    {
        return Some(partner.base_name.clone());
    }

    None
}

fn compute_ready(
    state: &SchedulerState,
    deps_map: &HashMap<String, HashSet<String>>,
    jobs: &HashMap<String, CompilationJob>,
) -> Vec<CompilationJob> {
    state
        .pending
        .iter()
        .filter_map(|base| {
            let deps = deps_map.get(base)?;
            if deps.iter().all(|d| state.finished.contains(d)) {
                jobs.get(base).cloned()
            } else {
                None
            }
        })
        .collect()
}

fn sort_ready(ready: &mut [CompilationJob]) {
    ready.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| thread_sort_key(b.threads).cmp(&thread_sort_key(a.threads)))
            .then_with(|| a.base_name.cmp(&b.base_name))
    });
}

fn thread_sort_key(threads: Option<usize>) -> usize {
    threads.unwrap_or(0)
}

fn pairing_caps(config: &Config) -> PairingCaps {
    match config.build.global_cpu_threads_mode.as_str() {
        "flexible" => PairingCaps {
            soft: config.build.global_cpu_threads_cap,
            hard: config.build.maximum_cpu_threads_cap,
        },
        _ => PairingCaps {
            soft: config.build.global_cpu_threads_cap,
            hard: config.build.global_cpu_threads_cap,
        },
    }
}

fn active_thread_sum(active: &[ActiveBuild]) -> usize {
    active.iter().map(|a| a.threads.unwrap_or(0)).sum()
}

fn can_start(
    job: &CompilationJob,
    active: &[ActiveBuild],
    caps: PairingCaps,
    slot_limit: usize,
    exclusive_alone: bool,
) -> bool {
    if exclusive_alone {
        return false;
    }
    if job.compile_alone && !active.is_empty() {
        return false;
    }
    if active.len() >= slot_limit {
        return false;
    }

    if job.compile_alone || active.is_empty() {
        return true;
    }

    let sum = active_thread_sum(active) + job.threads.unwrap_or(0);
    caps.hard.is_none_or(|h| sum <= h)
}

/// Best non-exclusive ready job whose thread count fits into `remaining`.
fn best_partner(ready: &[CompilationJob], remaining: usize) -> Option<&CompilationJob> {
    ready
        .iter()
        .filter(|j| !j.compile_alone && j.threads.unwrap_or(0) <= remaining)
        .max_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| thread_sort_key(a.threads).cmp(&thread_sort_key(b.threads)))
        })
}

fn find_partner<'a>(
    ready: &'a [CompilationJob],
    active: &[ActiveBuild],
    caps: PairingCaps,
) -> Option<&'a CompilationJob> {
    let used = active_thread_sum(active);
    for limit in [caps.soft, caps.hard] {
        let Some(limit) = limit else { continue };
        let remaining = limit.saturating_sub(used);
        if remaining == 0 {
            continue;
        }
        if let Some(p) = best_partner(ready, remaining) {
            return Some(p);
        }
    }
    // No hard ceiling configured: only the slot limit constrains concurrency.
    if caps.hard.is_none() {
        return best_partner(ready, usize::MAX);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(name: &str, threads: Option<usize>, alone: bool, priority: usize) -> CompilationJob {
        CompilationJob {
            base_name: name.to_string(),
            spec: PackageSpec::plain(name),
            threads,
            compile_alone: alone,
            priority,
        }
    }

    const CAPS_STRICT_10: PairingCaps = PairingCaps {
        soft: Some(10),
        hard: Some(10),
    };

    const CAPS_UNSET: PairingCaps = PairingCaps {
        soft: None,
        hard: None,
    };

    #[test]
    fn sort_ready_by_priority_then_threads() {
        let mut ready = vec![
            job("low", Some(8), false, 1),
            job("high", Some(4), false, 10),
            job("mid", Some(6), false, 5),
        ];
        sort_ready(&mut ready);
        assert_eq!(ready[0].base_name, "high");
        assert_eq!(ready[1].base_name, "mid");
        assert_eq!(ready[2].base_name, "low");
    }

    #[test]
    fn strict_pair_fits() {
        let active = vec![ActiveBuild {
            base_name: "a".into(),
            threads: Some(8),
        }];
        let candidate = job("b", Some(2), false, 1);
        assert!(can_start(
            &candidate,
            &active,
            CAPS_STRICT_10,
            2,
            false
        ));
    }

    #[test]
    fn strict_pair_rejected() {
        let active = vec![ActiveBuild {
            base_name: "a".into(),
            threads: Some(8),
        }];
        let candidate = job("b", Some(5), false, 1);
        assert!(!can_start(
            &candidate,
            &active,
            CAPS_STRICT_10,
            2,
            false
        ));
    }

    #[test]
    fn strict_no_partner_runs_solo() {
        let active: Vec<ActiveBuild> = vec![];
        let solo = job("a", Some(8), false, 1);
        assert!(can_start(&solo, &active, CAPS_STRICT_10, 2, false));
    }

    #[test]
    fn flexible_soft_then_max() {
        let caps = PairingCaps {
            soft: Some(10),
            hard: Some(15),
        };
        let active = vec![ActiveBuild {
            base_name: "a".into(),
            threads: Some(8),
        }];
        let ready = vec![job("b", Some(5), false, 1)];
        let partner = find_partner(&ready, &active, caps).unwrap();
        assert_eq!(partner.base_name, "b");
    }

    #[test]
    fn flexible_max_blocks() {
        let caps = PairingCaps {
            soft: Some(10),
            hard: Some(12),
        };
        let active = vec![ActiveBuild {
            base_name: "a".into(),
            threads: Some(8),
        }];
        let ready = vec![job("b", Some(5), false, 1)];
        assert!(find_partner(&ready, &active, caps).is_none());
    }

    #[test]
    fn no_caps_allows_partner_up_to_slot_limit() {
        let active = vec![ActiveBuild {
            base_name: "a".into(),
            threads: None,
        }];
        let ready = vec![job("b", None, false, 1)];
        let partner = find_partner(&ready, &active, CAPS_UNSET).unwrap();
        assert_eq!(partner.base_name, "b");
        assert!(can_start(&ready[0], &active, CAPS_UNSET, 2, false));
        // Slot limit still applies.
        assert!(!can_start(&ready[0], &active, CAPS_UNSET, 1, false));
    }

    #[test]
    fn flexible_soft_only_falls_back_to_unlimited() {
        let caps = PairingCaps {
            soft: Some(10),
            hard: None,
        };
        let active = vec![ActiveBuild {
            base_name: "a".into(),
            threads: Some(8),
        }];
        // Does not fit under the soft target, but no hard ceiling is configured.
        let ready = vec![job("b", Some(5), false, 1)];
        let partner = find_partner(&ready, &active, caps).unwrap();
        assert_eq!(partner.base_name, "b");
    }

    #[test]
    fn compile_alone_blocks_others() {
        let active = vec![ActiveBuild {
            base_name: "k".into(),
            threads: Some(8),
        }];
        let candidate = job("b", Some(2), false, 1);
        assert!(!can_start(
            &candidate,
            &active,
            CAPS_STRICT_10,
            2,
            true
        ));
    }

    #[test]
    fn compile_alone_waits_for_drain() {
        let active = vec![ActiveBuild {
            base_name: "a".into(),
            threads: Some(4),
        }];
        let alone = job("k", Some(8), true, 10);
        assert!(!can_start(
            &alone,
            &active,
            CAPS_STRICT_10,
            2,
            false
        ));
    }

    fn shared<'a>(
        jobs: &'a HashMap<String, CompilationJob>,
        deps_map: &'a HashMap<String, HashSet<String>>,
        caps: PairingCaps,
        slot_limit: usize,
    ) -> SchedulerShared<'a> {
        SchedulerShared {
            jobs,
            deps_map,
            caps,
            slot_limit,
        }
    }

    #[test]
    fn top_priority_alone_job_stops_partner_admission() {
        let jobs = HashMap::from([
            ("k".to_string(), job("k", Some(8), true, 10)),
            ("b".to_string(), job("b", Some(2), false, 1)),
        ]);
        let deps_map: HashMap<String, HashSet<String>> = jobs
            .keys()
            .map(|k| (k.clone(), HashSet::new()))
            .collect();
        let state = SchedulerState {
            pending: jobs.keys().cloned().collect(),
            active: vec![ActiveBuild {
                base_name: "a".into(),
                threads: Some(2),
            }],
            finished: HashSet::new(),
            failed: HashSet::new(),
            exclusive_alone: false,
        };
        // "b" would fit, but admitting it would starve the exclusive job "k".
        let sh = shared(&jobs, &deps_map, CAPS_STRICT_10, 3);
        assert!(pick_next_job(&state, &sh).is_none());
    }

    #[test]
    fn dependency_cycle_is_force_started_instead_of_deadlocking() {
        let jobs = HashMap::from([
            ("a".to_string(), job("a", Some(2), false, 1)),
            ("b".to_string(), job("b", Some(2), false, 5)),
        ]);
        let deps_map = HashMap::from([
            ("a".to_string(), HashSet::from(["b".to_string()])),
            ("b".to_string(), HashSet::from(["a".to_string()])),
        ]);
        let state = SchedulerState {
            pending: jobs.keys().cloned().collect(),
            active: vec![],
            finished: HashSet::new(),
            failed: HashSet::new(),
            exclusive_alone: false,
        };
        let sh = shared(&jobs, &deps_map, CAPS_STRICT_10, 2);
        assert!(pick_next_job(&state, &sh).is_none());
        // Higher-priority cycle member is force-started.
        assert_eq!(force_pick_if_stalled(&state, &sh), Some("b".to_string()));
    }

    #[test]
    fn no_force_pick_while_builds_are_active() {
        let jobs = HashMap::from([("a".to_string(), job("a", Some(2), false, 1))]);
        let deps_map = HashMap::from([(
            "a".to_string(),
            HashSet::from(["x".to_string()]),
        )]);
        let state = SchedulerState {
            pending: jobs.keys().cloned().collect(),
            active: vec![ActiveBuild {
                base_name: "x".into(),
                threads: Some(2),
            }],
            finished: HashSet::new(),
            failed: HashSet::new(),
            exclusive_alone: false,
        };
        let sh = shared(&jobs, &deps_map, CAPS_STRICT_10, 2);
        assert!(force_pick_if_stalled(&state, &sh).is_none());
    }

    #[test]
    fn dependency_blocks_ready_until_finished() {
        let state = SchedulerState {
            pending: ["b".to_string()].into_iter().collect(),
            active: vec![],
            finished: ["a".to_string()].into_iter().collect(),
            failed: HashSet::new(),
            exclusive_alone: false,
        };
        let mut deps = HashMap::new();
        deps.insert("b".to_string(), ["a".to_string()].into_iter().collect());
        let jobs = HashMap::from([("b".to_string(), job("b", Some(2), false, 1))]);
        let ready = compute_ready(&state, &deps, &jobs);
        assert_eq!(ready.len(), 1);

        let mut blocked = state.clone();
        blocked.pending.insert("c".to_string());
        blocked.finished.remove("a");
        blocked.failed.insert("a".to_string());
        deps.insert("c".to_string(), ["a".to_string()].into_iter().collect());
        let mut blocked_state = blocked;
        drain_blocked_pending(&mut blocked_state, &deps);
        assert!(blocked_state.failed.contains("c"));
        assert!(!blocked_state.pending.contains("c"));
    }
}
