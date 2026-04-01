//! File change watcher: watches for file changes and fires FileChanged/CwdChanged hooks.
//!
//! Mirrors `fileChangedWatcher.ts`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use notify::Watcher;
use tracing::{debug, warn};

use super::execution::execute_hooks_outside_repl;
use super::registry::HookRegistry;
use super::types::*;

/// Callback for notifications from the file watcher.
pub type WatcherNotifyCallback = Box<dyn Fn(&str, bool) + Send + Sync>;

/// File change watcher that monitors files and fires hooks when they change.
pub struct FileChangedWatcher {
    inner: Arc<Mutex<WatcherInner>>,
}

struct WatcherInner {
    watcher: Option<notify::RecommendedWatcher>,
    current_cwd: PathBuf,
    dynamic_watch_paths: Vec<String>,
    initialized: bool,
    notify_callback: Option<WatcherNotifyCallback>,
}

impl FileChangedWatcher {
    /// Create a new file change watcher (not yet initialized).
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(WatcherInner {
                watcher: None,
                current_cwd: PathBuf::new(),
                dynamic_watch_paths: Vec::new(),
                initialized: false,
                notify_callback: None,
            })),
        }
    }

    /// Set the notification callback for watcher events.
    pub fn set_notify_callback(&self, cb: Option<WatcherNotifyCallback>) {
        let mut inner = self.inner.lock().unwrap();
        inner.notify_callback = cb;
    }

    /// Initialize the watcher with the given working directory and hook config.
    ///
    /// Must be called once during startup. If no FileChanged or CwdChanged hooks
    /// are configured, the watcher does nothing.
    pub fn initialize(
        &self,
        cwd: &Path,
        hooks_config: &HooksSettings,
        registry: Arc<HookRegistry>,
        base_input: BaseHookInput,
    ) {
        let mut inner = self.inner.lock().unwrap();
        if inner.initialized {
            return;
        }
        inner.initialized = true;
        inner.current_cwd = cwd.to_path_buf();

        let has_env_hooks = hooks_config.contains_key("CwdChanged")
            || hooks_config.contains_key("FileChanged");

        if !has_env_hooks {
            return;
        }

        let paths = resolve_watch_paths(hooks_config, cwd, &inner.dynamic_watch_paths);
        if paths.is_empty() {
            return;
        }

        start_watching(&mut inner, paths, registry, base_input);
    }

    /// Update the dynamic watch paths (from hook output).
    pub fn update_watch_paths(
        &self,
        paths: Vec<String>,
        hooks_config: &HooksSettings,
        registry: Arc<HookRegistry>,
        base_input: BaseHookInput,
    ) {
        let mut inner = self.inner.lock().unwrap();
        if !inner.initialized {
            return;
        }

        let mut sorted = paths.clone();
        sorted.sort();

        let mut existing_sorted = inner.dynamic_watch_paths.clone();
        existing_sorted.sort();

        if sorted == existing_sorted {
            return;
        }

        inner.dynamic_watch_paths = paths;
        restart_watching(&mut inner, hooks_config, registry, base_input);
    }

    /// Handle a change in the current working directory.
    pub async fn on_cwd_changed(
        &self,
        old_cwd: &str,
        new_cwd: &str,
        registry: &HookRegistry,
        base_input: BaseHookInput,
        hooks_config: &HooksSettings,
    ) -> Vec<HookOutsideReplResult> {
        if old_cwd == new_cwd {
            return Vec::new();
        }

        let has_env_hooks = hooks_config.contains_key("CwdChanged")
            || hooks_config.contains_key("FileChanged");

        if !has_env_hooks {
            return Vec::new();
        }

        {
            let mut inner = self.inner.lock().unwrap();
            inner.current_cwd = PathBuf::from(new_cwd);
        }

        let input = HookInput::CwdChanged {
            base: base_input,
            old_cwd: old_cwd.to_string(),
            new_cwd: new_cwd.to_string(),
        };

        let results = execute_hooks_outside_repl(registry, &input, None).await;

        // Collect watch paths from results
        let watch_paths: Vec<String> = results
            .iter()
            .flat_map(|r| r.watch_paths.iter().cloned())
            .collect();

        {
            let mut inner = self.inner.lock().unwrap();
            inner.dynamic_watch_paths = watch_paths;
        }

        results
    }

    /// Dispose of the watcher and clean up resources.
    pub fn dispose(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.watcher = None;
        inner.dynamic_watch_paths.clear();
        inner.initialized = false;
        inner.notify_callback = None;
    }
}

impl Default for FileChangedWatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve the full set of paths to watch.
///
/// Combines static paths from FileChanged matcher patterns with dynamic paths
/// from hook output.
fn resolve_watch_paths(
    config: &HooksSettings,
    cwd: &Path,
    dynamic_paths: &[String],
) -> Vec<PathBuf> {
    let matchers = config.get("FileChanged").cloned().unwrap_or_default();
    let mut static_paths = Vec::new();

    for matcher in &matchers {
        if let Some(pattern) = &matcher.matcher {
            // Matcher field: filenames to watch, pipe-separated (e.g., ".envrc|.env")
            for name in pattern.split('|').map(|s| s.trim()) {
                if name.is_empty() {
                    continue;
                }
                let path = if Path::new(name).is_absolute() {
                    PathBuf::from(name)
                } else {
                    cwd.join(name)
                };
                static_paths.push(path);
            }
        }
    }

    // Combine static + dynamic, deduplicated
    let mut all_paths: Vec<PathBuf> = static_paths;
    for dp in dynamic_paths {
        let p = PathBuf::from(dp);
        if !all_paths.contains(&p) {
            all_paths.push(p);
        }
    }

    all_paths
}

/// Start watching the given paths for changes.
fn start_watching(
    inner: &mut WatcherInner,
    paths: Vec<PathBuf>,
    registry: Arc<HookRegistry>,
    base_input: BaseHookInput,
) {
    debug!("FileChanged: watching {} paths", paths.len());

    let registry = registry.clone();
    let base_input = base_input.clone();

    let watcher_result = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        match event {
            Ok(event) => {
                let change_type = match event.kind {
                    notify::EventKind::Create(_) => "add",
                    notify::EventKind::Modify(_) => "change",
                    notify::EventKind::Remove(_) => "unlink",
                    _ => return,
                };

                for path in &event.paths {
                    debug!("FileChanged: {change_type} {}", path.display());
                    let path_str = path.to_string_lossy().to_string();
                    let input = HookInput::FileChanged {
                        base: base_input.clone(),
                        file_path: path_str,
                        change_type: change_type.to_string(),
                    };

                    // Fire-and-forget: spawn the hook execution
                    let reg = registry.clone();
                    tokio::spawn(async move {
                        let _ = execute_hooks_outside_repl(&reg, &input, None).await;
                    });
                }
            }
            Err(e) => {
                warn!("file watcher error: {e}");
            }
        }
    });

    match watcher_result {
        Ok(mut watcher) => {
            for path in &paths {
                if let Err(e) = watcher.watch(path, notify::RecursiveMode::NonRecursive) {
                    debug!("FileChanged: failed to watch {}: {e}", path.display());
                }
            }
            inner.watcher = Some(watcher);
        }
        Err(e) => {
            warn!("failed to create file watcher: {e}");
        }
    }
}

/// Restart watching with the current configuration.
fn restart_watching(
    inner: &mut WatcherInner,
    config: &HooksSettings,
    registry: Arc<HookRegistry>,
    base_input: BaseHookInput,
) {
    // Drop the old watcher
    inner.watcher = None;

    let paths = resolve_watch_paths(config, &inner.current_cwd, &inner.dynamic_watch_paths);
    if !paths.is_empty() {
        start_watching(inner, paths, registry, base_input);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_watch_paths_static() {
        let mut config = HooksSettings::new();
        config.insert(
            "FileChanged".to_string(),
            vec![HookMatcher {
                matcher: Some(".envrc|.env".to_string()),
                hooks: vec![],
            }],
        );

        let cwd = Path::new("/tmp/project");
        let paths = resolve_watch_paths(&config, cwd, &[]);
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&cwd.join(".envrc")));
        assert!(paths.contains(&cwd.join(".env")));
    }

    #[test]
    fn test_resolve_watch_paths_absolute() {
        let mut config = HooksSettings::new();
        config.insert(
            "FileChanged".to_string(),
            vec![HookMatcher {
                matcher: Some("/etc/config.yaml".to_string()),
                hooks: vec![],
            }],
        );

        let cwd = Path::new("/tmp/project");
        let paths = resolve_watch_paths(&config, cwd, &[]);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], PathBuf::from("/etc/config.yaml"));
    }

    #[test]
    fn test_resolve_watch_paths_with_dynamic() {
        let config = HooksSettings::new();
        let cwd = Path::new("/tmp/project");
        let dynamic = vec!["/tmp/dynamic.conf".to_string()];
        let paths = resolve_watch_paths(&config, cwd, &dynamic);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], PathBuf::from("/tmp/dynamic.conf"));
    }
}
