// SPDX-License-Identifier: MIT

use std::process::{Child, ExitStatus};
use std::time::{Duration, Instant};

use crate::model::{CommandSpec, Service};
use crate::unix::child::{ChildError, ChildSpawnOptions, OsCommandSpec, spawn};

pub struct ActionManager {
    options: ChildSpawnOptions,
    children: Vec<Child>,
}

impl ActionManager {
    pub fn new(options: ChildSpawnOptions) -> Self {
        Self {
            options,
            children: Vec::new(),
        }
    }

    pub fn spawn(&mut self, command: &CommandSpec) -> Result<(), ChildError> {
        let spec = OsCommandSpec::from_model(command);
        let child = spawn(&spec, &self.options)?;
        self.children.push(child);
        self.reap();
        Ok(())
    }

    pub fn reap(&mut self) {
        self.children.retain_mut(|child| {
            match child.try_wait() {
                Ok(Some(_status)) => false,
                Ok(None) => true,
                Err(_) => false, // if try_wait fails, just drop the child, not much we can do
            }
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    Running,
    Terminating {
        deadline: std::time::Instant,
    },
    Killed,
    Exited,
}

#[derive(Debug)]
pub struct ServiceChild {
    name: String,
    child: Child,
    state: ServiceState,
}

impl ServiceChild {
    pub fn new(name: String, child: Child) -> Self {
        Self {
            name,
            child,
            state: ServiceState::Running,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn state(&self) -> ServiceState {
        self.state
    }

    pub fn is_done(&self) -> bool {
        matches!(self.state, ServiceState::Exited)
    }

    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        if status.is_some() {
            self.state = ServiceState::Exited;
        }
        Ok(status)
    }

    pub fn begin_terminate(&mut self, deadline: Instant) -> std::io::Result<()> {
        if matches!(self.state, ServiceState::Exited | ServiceState::Killed) {
            return Ok(());
        }

        self.try_wait()?;

        if matches!(self.state, ServiceState::Exited) {
            return Ok(());
        }

        signal_child(&self.child, libc::SIGTERM)?;
        self.state = ServiceState::Terminating { deadline };
        Ok(())
    }

    pub fn kill_now(&mut self) -> std::io::Result<()> {
        if matches!(self.state, ServiceState::Exited | ServiceState::Killed) {
            return Ok(());
        }

        self.try_wait()?;

        if matches!(self.state, ServiceState::Exited) {
            return Ok(());
        }

        self.child.kill()?;
        self.state = ServiceState::Killed;
        Ok(())
    }
}

fn signal_child(child: &Child, signal: libc::c_int) -> std::io::Result<()> {
    let pid = child.id() as libc::pid_t;
    if unsafe { libc::kill(pid, signal) } < 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        return Err(err);
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("failed to spawn service '{name}': {source}")]
    Spawn { name: String, #[source] source: ChildError, cleanup: Vec<ServiceCleanupError> },

    #[error("service '{name}' exited unexpectedly with status {status}")]
    UnexpectedExit { name: String, status: std::process::ExitStatus },

    #[error("failed to check service '{name}': {source}")]
    Check { name: String, #[source] source: std::io::Error },

    #[error("failed to terminate service '{name}': {source}")]
    Terminate { name: String, #[source] source: std::io::Error },

    #[error("failed to kill service '{name}': {source}")]
    Kill { name: String, #[source] source: std::io::Error },
}

#[derive(Debug, thiserror::Error)]
pub enum ServiceCleanupError {
    #[error("failed to check service '{name}': {source}")]
    Check { name: String, #[source] source: std::io::Error },

    #[error("failed to terminate service '{name}': {source}")]
    Terminate { name: String, #[source] source: std::io::Error },

    #[error("failed to kill service '{name}': {source}")]
    Kill { name: String, #[source] source: std::io::Error },
}

impl ServiceCleanupError {
    pub fn try_from(err: ServiceError) -> Option<ServiceCleanupError> {
        match err {
            ServiceError::Check { name, source } => Some(ServiceCleanupError::Check { name, source }),
            ServiceError::Terminate { name, source } => Some(ServiceCleanupError::Terminate { name, source }),
            ServiceError::Kill { name, source } => Some(ServiceCleanupError::Kill { name, source }),
            _ => None
        }
    }
}

pub struct ServiceManager {
    services: Vec<ServiceChild>,
    shutdown_grace: Duration,
    shutting_down: bool,
}

impl ServiceManager {
    pub fn start(services: &Vec<Service>, spawn_options: ChildSpawnOptions, shutdown_grace: Duration) -> Result<Self, ServiceError> {
        let mut manager = Self {
            services: Vec::new(),
            shutdown_grace,
            shutting_down: false
        };

        for sv in services {
            let spec = OsCommandSpec::from_model(&sv.command);
            match spawn(&spec, &spawn_options) {
                Ok(child) => manager.services.push(ServiceChild::new(sv.name.clone(), child)),
                Err(source) => {
                    let now = Instant::now();
                    let mut errors = Vec::new();

                    manager.shutting_down = true;
                    let deadline = now + manager.shutdown_grace;

                    for sv in &mut manager.services {
                        let name = sv.name().to_owned();
                        if let Err(source) = sv.begin_terminate(deadline) {
                            errors.push(ServiceCleanupError::Terminate { name, source });
                        }
                    }

                    while !manager.services.is_empty() && Instant::now() < deadline {
                        for sv in &mut manager.services {
                            let name = sv.name().to_owned();
                            if let Err(source) = sv.try_wait() {
                                errors.push(ServiceCleanupError::Check { name, source });
                            }
                        }
                        manager.services.retain(|sv| !sv.is_done());
                        if !manager.services.is_empty() {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                    }

                    for service in &mut manager.services {
                        if service.is_done() {
                            continue;
                        }
                        let name = service.name().to_owned();
                        if let Err(source) = service.kill_now() {
                            errors.push(ServiceCleanupError::Kill { name, source });
                        }
                    }
                    manager.services.clear();

                    return Err(ServiceError::Spawn {
                        name: sv.name.clone(),
                        source,
                        cleanup: errors
                    });
                }
            }
        }

        Ok(manager)
    }

    pub fn check_exits(&mut self) -> Result<(), ServiceError> {
        for sv in &mut self.services {
            let name = sv.name().to_owned();
            if !self.shutting_down && let Some(status) = sv.try_wait().map_err(|source| ServiceError::Check {
                name: name.clone(),
                source,
            })? {
                return Err(ServiceError::UnexpectedExit { name, status });
            }
        }
        self.services.retain(|sv| !sv.is_done());
        Ok(())
    }

    pub fn begin_shutdown(&mut self, now: Instant) -> Result<(), ServiceError> {
        if self.shutting_down {
            return Ok(());
        }
        self.shutting_down = true;
        let deadline = now + self.shutdown_grace;
        for sv in &mut self.services {
            let name = sv.name().to_owned();
            sv.begin_terminate(deadline).map_err(|source| ServiceError::Terminate {
                name,
                source,
            })?;
        }
        Ok(())
    }

    pub fn poll_shutdown(&mut self, now: Instant) -> Result<(), ServiceError> {
        if self.is_shutdown_complete() {
            return Ok(());
        }
        self.check_exits()?;
        for sv in &mut self.services {
            let ServiceState::Terminating { deadline } = sv.state() else {
                continue;
            };

            if now < deadline {
                continue;
            }

            sv.kill_now().map_err(|source| ServiceError::Kill {
                name: sv.name().to_owned(),
                source,
            })?;
        }
        self.check_exits()?;
        Ok(())
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.services.iter().filter_map(|service| match service.state() {
            ServiceState::Terminating { deadline } => Some(deadline),
            _ => None,
        }).min()
    }

    pub fn is_shutdown_complete(&self) -> bool {
        self.services.is_empty()
    }

    pub fn kill_all_now(&mut self) {
        self.shutting_down = true;
        for service in &mut self.services {
            _ = service.kill_now();
        }
    }
}

impl Drop for ServiceManager {
    fn drop(&mut self) {
        self.kill_all_now();
    }
}
