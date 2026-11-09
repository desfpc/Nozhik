//! Compose evaluation, immutable container targeting and directory ownership checks.
use crate::{Action, Container, Error, Response, Result, Service, Snapshot};
use serde_json::Value;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
pub struct Engine {
    root: PathBuf,
    program: PathBuf,
    cancel: Arc<AtomicBool>,
}
impl Engine {
    pub const fn new(root: PathBuf, program: PathBuf, cancel: Arc<AtomicBool>) -> Self {
        Self {
            root,
            program,
            cancel,
        }
    }
    fn run(&self, cwd: &Path, args: &[OsString], timeout: Duration) -> Result<String> {
        crate::process::run(&self.program, cwd, args, &self.cancel, timeout)
    }
    fn config_args(&self, files: &[PathBuf]) -> Result<(PathBuf, Vec<OsString>)> {
        let root = self.root.canonicalize()?;
        let first = files
            .first()
            .ok_or_else(|| Error::Message("Select a Compose file".into()))?;
        let directory = first
            .parent()
            .ok_or_else(|| Error::Message("Compose file has no directory".into()))?
            .canonicalize()?;
        let mut args: Vec<OsString> = vec![
            "compose".into(),
            "--project-directory".into(),
            directory.clone().into_os_string(),
        ];
        for file in files {
            let file = file.canonicalize()?;
            if !file.starts_with(&root) || !file.is_file() || !crate::is_compose_file(&file) {
                return Err(Error::Message(
                    "Compose configuration must belong to the current IDE project".into(),
                ));
            }
            args.extend(["-f".into(), file.into_os_string()]);
        }
        Ok((directory, args))
    }
    pub fn snapshot(&self, files: &[PathBuf]) -> Result<Snapshot> {
        let (directory, mut args) = self.config_args(files)?;
        args.extend(["config".into(), "--format".into(), "json".into()]);
        let config: Value =
            serde_json::from_str(&self.run(&directory, &args, Duration::from_secs(30))?)?;
        let project = config["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                Error::Message(
                    "Docker Compose did not return a project name; update the Compose plugin"
                        .into(),
                )
            })?
            .to_owned();
        let services = config["services"]
            .as_object()
            .ok_or_else(|| Error::Message("Compose configuration has no services".into()))?
            .iter()
            .map(|(name, value)| Service {
                name: name.clone(),
                image: value["image"].as_str().unwrap_or("").into(),
            })
            .collect();
        let args = strings(&[
            "ps",
            "--all",
            "--filter",
            &format!("label=com.docker.compose.project={project}"),
            "--format",
            "json",
        ]);
        let output = self.run(&directory, &args, Duration::from_secs(15))?;
        let rows = json_rows(&output)?;
        let mut containers = Vec::new();
        if !rows.is_empty() {
            let mut args = strings(&["inspect", "--type", "container", "--"]);
            for row in rows {
                args.push(valid_id(row["ID"].as_str().unwrap_or(""))?.into());
            }
            let inspected = self.run(&directory, &args, Duration::from_secs(15))?;
            for value in json_rows(&inspected)? {
                Self::check_scope(&value, &directory, &project)?;
                containers.push(container(&value)?);
            }
        }
        containers.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Snapshot {
            project,
            services,
            containers,
        })
    }
    fn check_scope(value: &Value, directory: &Path, project: &str) -> Result<()> {
        let labels = &value["Config"]["Labels"];
        let working = labels["com.docker.compose.project.working_dir"]
            .as_str()
            .map(Path::new)
            .and_then(|p| p.canonicalize().ok());
        if working.as_deref() != Some(directory)
            || labels["com.docker.compose.project"].as_str() != Some(project)
        {
            return Err(Error::Message("Compose project name is also used by containers from another directory. Choose a unique top-level name: before controlling this configuration.".into()));
        }
        Ok(())
    }
    pub fn act(&self, files: &[PathBuf], action: &Action) -> Result<Response> {
        let snapshot = self.snapshot(files)?;
        let (directory, mut compose) = self.config_args(files)?;
        match action {
            Action::Up(service) => {
                if let Some(service) = service
                    && (!crate::compose::valid_service(service)
                        || !snapshot.services.iter().any(|s| &s.name == service))
                {
                    return Err(Error::Message(
                        "The Compose service is no longer declared".into(),
                    ));
                }
                compose.extend(strings(&["up", "--detach", "--"]));
                compose.extend(service.as_ref().map(OsString::from));
                self.run(&directory, &compose, Duration::from_secs(600))?;
            }
            Action::StopAll => {
                if !snapshot.containers.is_empty() {
                    let mut args = strings(&["stop", "--time", "10", "--"]);
                    args.extend(snapshot.containers.iter().map(|c| OsString::from(&c.id)));
                    self.run(&directory, &args, Duration::from_secs(120))?;
                }
            }
            Action::Start(id) | Action::Stop(id) | Action::Logs(id) | Action::Console(id) => {
                let target = snapshot
                    .containers
                    .iter()
                    .find(|c| &c.id == id)
                    .ok_or_else(|| {
                        Error::Message(
                            "Container is no longer part of this project; refresh the list".into(),
                        )
                    })?;
                if let Some(response) = self.container_action(&directory, target, action)? {
                    return Ok(response);
                }
            }
        }
        self.snapshot(files).map(Response::Snapshot)
    }
    fn container_action(
        &self,
        directory: &Path,
        target: &Container,
        action: &Action,
    ) -> Result<Option<Response>> {
        match action {
            Action::Logs(_) => {
                let text = self.run(
                    directory,
                    &strings(&["logs", "--tail", "500", "--timestamps", "--", &target.id]),
                    Duration::from_secs(15),
                )?;
                Ok(Some(Response::Logs(text)))
            }
            Action::Console(_) => {
                if target.state != "running" {
                    return Err(Error::Message(
                        "Start the container before opening its console".into(),
                    ));
                }
                Ok(Some(Response::Console {
                    program: self.program.clone(),
                    args: strings(&["exec", "-it", "--", &target.id, "/bin/sh"]),
                    name: target.name.clone(),
                    id: target.id.clone(),
                }))
            }
            Action::Start(_) | Action::Stop(_) => {
                let verb = if matches!(action, Action::Start(_)) {
                    "start"
                } else {
                    "stop"
                };
                self.run(
                    directory,
                    &strings(&[verb, "--", &target.id]),
                    Duration::from_secs(120),
                )?;
                Ok(None)
            }
            _ => Err(Error::Message("Invalid container action".into())),
        }
    }
}
fn strings(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}
pub fn valid_id(id: &str) -> Result<&str> {
    if !(12..=64).contains(&id.len()) || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Message(
            "Docker returned an invalid container ID".into(),
        ));
    }
    Ok(id)
}
pub fn json_rows(source: &str) -> Result<Vec<Value>> {
    if source.trim().is_empty() {
        return Ok(Vec::new());
    }
    if source.trim_start().starts_with('[') {
        return Ok(serde_json::from_str(source)?);
    }
    source
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(Error::from))
        .collect()
}
fn container(value: &Value) -> Result<Container> {
    let string = |v: &Value| v.as_str().unwrap_or("").to_owned();
    let labels = value["Config"]["Labels"]
        .as_object()
        .map(|labels| labels.iter().map(|(k, v)| (k.clone(), string(v))).collect())
        .unwrap_or_default();
    let mounts = value["Mounts"]
        .as_array()
        .map(|mounts| {
            mounts
                .iter()
                .map(|m| (string(&m["Source"]), string(&m["Destination"])))
                .collect()
        })
        .unwrap_or_default();
    Ok(Container {
        id: valid_id(value["Id"].as_str().unwrap_or(""))?.into(),
        name: string(&value["Name"]).trim_start_matches('/').into(),
        service: string(&value["Config"]["Labels"]["com.docker.compose.service"]),
        image: string(&value["Config"]["Image"]),
        state: string(&value["State"]["Status"]),
        health: string(&value["State"]["Health"]["Status"]),
        exit_code: value["State"]["ExitCode"].as_i64().unwrap_or(0),
        mounts,
        labels,
    })
}
