//! Watches application registration surfaces and refreshes only app entries.

use std::time::{Duration, Instant};

const INSTALLER_DEBOUNCE: Duration = Duration::from_secs(2);
#[cfg(target_os = "windows")]
const RECONCILE_INTERVAL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug)]
struct AppChangeDebouncer {
    due_at: Option<Instant>,
}

impl AppChangeDebouncer {
    fn new() -> Self {
        Self { due_at: None }
    }

    fn record(&mut self, now: Instant) {
        self.due_at = Some(now + INSTALLER_DEBOUNCE);
    }

    fn take_if_due(&mut self, now: Instant) -> bool {
        if self.due_at.is_some_and(|due_at| now >= due_at) {
            self.due_at = None;
            true
        } else {
            false
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::{
        ffi::c_void,
        sync::{
            Arc, RwLock,
            atomic::{AtomicBool, Ordering},
            mpsc::{self, Receiver, Sender},
        },
        thread::{self, JoinHandle},
        time::{Duration, Instant},
    };

    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, WAIT_OBJECT_0},
        System::{
            Registry::{
                HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY,
                REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_CHANGE_NAME, RegCloseKey,
                RegNotifyChangeKeyValue, RegOpenKeyExW,
            },
            Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects},
        },
    };

    use super::{AppChangeDebouncer, RECONCILE_INTERVAL};
    use crate::{
        core_service::CoreService,
        discovery::application_shortcut_roots,
        file_watcher::{DirectoryWatcher, WatcherConfig, WatcherEvent},
    };

    const POLL_INTERVAL: Duration = Duration::from_millis(100);
    const RETRY_DELAY: Duration = Duration::from_secs(5);

    struct RegistryWatchSpec {
        root: HKEY,
        subkey: &'static str,
        view_flags: u32,
    }

    struct RegistryWatcher {
        stop_event: HANDLE,
        worker: Option<JoinHandle<()>>,
    }

    // The handle is only signaled and closed by its owner during shutdown.
    unsafe impl Send for RegistryWatcher {}

    impl RegistryWatcher {
        fn start(spec: RegistryWatchSpec, changes: Sender<()>) -> Result<Self, String> {
            let stop_event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
            if stop_event.is_null() {
                return Err("CreateEventW failed for registry watcher stop event".into());
            }

            let root = spec.root as usize;
            let worker_stop_event = stop_event as usize;
            let worker = thread::Builder::new()
                .name("nex-app-registry-watch".into())
                .spawn(move || {
                    let root = root as *mut c_void;
                    let stop_event = worker_stop_event as HANDLE;
                    let subkey = wide(spec.subkey);
                    let mut key: HKEY = std::ptr::null_mut();
                    let status = unsafe {
                        RegOpenKeyExW(root, subkey.as_ptr(), 0, KEY_READ | spec.view_flags, &mut key)
                    };
                    if status != ERROR_SUCCESS {
                        crate::logging::warn(&format!(
                            "[nex] app registry watcher could not open {} (status={status})",
                            spec.subkey
                        ));
                        return;
                    }

                    let change_event =
                        unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
                    if change_event.is_null() {
                        unsafe { RegCloseKey(key) };
                        crate::logging::warn("[nex] app registry watcher could not create change event");
                        return;
                    }

                    loop {
                        let status = unsafe {
                            RegNotifyChangeKeyValue(
                                key,
                                1,
                                REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_CHANGE_LAST_SET,
                                change_event,
                                1,
                            )
                        };
                        if status != ERROR_SUCCESS {
                            crate::logging::warn(&format!(
                                "[nex] app registry watcher notification failed for {} (status={status})",
                                spec.subkey
                            ));
                            break;
                        }

                        let handles = [stop_event, change_event];
                        let result = unsafe {
                            WaitForMultipleObjects(
                                handles.len() as u32,
                                handles.as_ptr(),
                                0,
                                INFINITE,
                            )
                        };
                        if result == WAIT_OBJECT_0 {
                            break;
                        }
                        if result == WAIT_OBJECT_0 + 1 {
                            let _ = changes.send(());
                        }
                    }

                    unsafe {
                        CloseHandle(change_event);
                        RegCloseKey(key);
                    }
                })
                .map_err(|error| format!("registry watcher thread start failed: {error}"))?;

            Ok(Self {
                stop_event,
                worker: Some(worker),
            })
        }

        fn stop(&mut self) {
            unsafe {
                SetEvent(self.stop_event);
            }
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
            unsafe {
                CloseHandle(self.stop_event);
            }
            self.stop_event = std::ptr::null_mut();
        }
    }

    impl Drop for RegistryWatcher {
        fn drop(&mut self) {
            if !self.stop_event.is_null() {
                self.stop();
            }
        }
    }

    /// Owns native Start Menu, Desktop, and registry change watchers.
    pub(crate) struct ApplicationWatcherHandle {
        stop: Arc<AtomicBool>,
        directory_watchers: Vec<DirectoryWatcher>,
        registry_watchers: Vec<RegistryWatcher>,
        worker: Option<JoinHandle<()>>,
    }

    impl ApplicationWatcherHandle {
        pub(crate) fn start(service: Arc<RwLock<CoreService>>) -> Self {
            let stop = Arc::new(AtomicBool::new(false));
            let (registry_tx, registry_rx) = mpsc::channel();
            let mut directory_watchers = Vec::new();
            let mut directory_receivers = Vec::new();

            for root in application_shortcut_roots() {
                match DirectoryWatcher::start(WatcherConfig::new(root.clone(), Vec::new())) {
                    Ok((watcher, receiver)) => {
                        directory_watchers.push(watcher);
                        directory_receivers.push(receiver);
                    }
                    Err(error) => crate::logging::warn(&format!(
                        "[nex] app directory watcher skipped {}: {error}",
                        root.display()
                    )),
                }
            }

            let mut registry_watchers = Vec::new();
            for spec in registry_watch_specs() {
                match RegistryWatcher::start(spec, registry_tx.clone()) {
                    Ok(watcher) => registry_watchers.push(watcher),
                    Err(error) => crate::logging::warn(&format!(
                        "[nex] app registry watcher skipped: {error}"
                    )),
                }
            }
            drop(registry_tx);

            let worker_stop = Arc::clone(&stop);
            let worker = thread::Builder::new()
                .name("nex-app-discovery-watch".into())
                .spawn(move || {
                    run_refresh_loop(worker_stop, directory_receivers, registry_rx, service)
                })
                .ok();

            Self {
                stop,
                directory_watchers,
                registry_watchers,
                worker,
            }
        }

        pub(crate) fn active_directories(&self) -> usize {
            self.directory_watchers.len()
        }

        pub(crate) fn active_registry_keys(&self) -> usize {
            self.registry_watchers.len()
        }
    }

    impl Drop for ApplicationWatcherHandle {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            for watcher in &mut self.directory_watchers {
                watcher.stop();
            }
            for watcher in &mut self.registry_watchers {
                watcher.stop();
            }
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    fn run_refresh_loop(
        stop: Arc<AtomicBool>,
        directory_receivers: Vec<Receiver<Vec<WatcherEvent>>>,
        registry_receiver: Receiver<()>,
        service: Arc<RwLock<CoreService>>,
    ) {
        let mut debouncer = AppChangeDebouncer::new();
        let mut reconcile_at = Instant::now() + RECONCILE_INTERVAL;
        let mut retry_at = None;

        while !stop.load(Ordering::Acquire) {
            let now = Instant::now();
            let mut changed = false;
            for receiver in &directory_receivers {
                while receiver.try_recv().is_ok() {
                    changed = true;
                }
            }
            while registry_receiver.try_recv().is_ok() {
                changed = true;
            }
            if changed {
                debouncer.record(now);
            }

            let should_reconcile = now >= reconcile_at;
            let should_retry = retry_at.is_some_and(|at| now >= at);
            if debouncer.take_if_due(now) || should_reconcile || should_retry {
                retry_at = None;
                reconcile_at = now + RECONCILE_INTERVAL;
                match service.try_read() {
                    Ok(service) => match service.rebuild_application_index_with_report() {
                        Ok(report) => crate::logging::info(&format!(
                            "[nex] app discovery refresh: discovered={} upserted={} removed={}",
                            report.discovered_total, report.upserted_total, report.removed_total
                        )),
                        Err(error) => {
                            crate::logging::warn(&format!(
                                "[nex] app discovery refresh failed; retrying: {error}"
                            ));
                            retry_at = Some(now + RETRY_DELAY);
                        }
                    },
                    Err(_) => retry_at = Some(now + RETRY_DELAY),
                }
            }

            thread::sleep(POLL_INTERVAL);
        }
    }

    fn registry_watch_specs() -> Vec<RegistryWatchSpec> {
        const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
        const APP_PATHS: &str = r"Software\Microsoft\Windows\CurrentVersion\App Paths";
        vec![
            RegistryWatchSpec {
                root: HKEY_CURRENT_USER,
                subkey: UNINSTALL,
                view_flags: 0,
            },
            RegistryWatchSpec {
                root: HKEY_LOCAL_MACHINE,
                subkey: UNINSTALL,
                view_flags: 0,
            },
            RegistryWatchSpec {
                root: HKEY_LOCAL_MACHINE,
                subkey: UNINSTALL,
                view_flags: KEY_WOW64_32KEY,
            },
            RegistryWatchSpec {
                root: HKEY_CURRENT_USER,
                subkey: APP_PATHS,
                view_flags: 0,
            },
            RegistryWatchSpec {
                root: HKEY_LOCAL_MACHINE,
                subkey: APP_PATHS,
                view_flags: 0,
            },
            RegistryWatchSpec {
                root: HKEY_LOCAL_MACHINE,
                subkey: APP_PATHS,
                view_flags: KEY_WOW64_32KEY,
            },
        ]
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(target_os = "windows")]
pub(crate) use windows::ApplicationWatcherHandle;

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{AppChangeDebouncer, INSTALLER_DEBOUNCE};

    #[test]
    fn debounces_installer_bursts_until_the_latest_change_is_quiet() {
        let start = Instant::now();
        let mut debouncer = AppChangeDebouncer::new();

        debouncer.record(start);
        debouncer.record(start + Duration::from_millis(500));

        assert!(!debouncer.take_if_due(start + INSTALLER_DEBOUNCE));
        assert!(debouncer.take_if_due(start + Duration::from_millis(500) + INSTALLER_DEBOUNCE));
        assert!(!debouncer.take_if_due(start + Duration::from_secs(10)));
    }
}
