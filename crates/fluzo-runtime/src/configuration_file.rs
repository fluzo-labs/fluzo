use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use fluzo_core::configuration::ConfigurationError;

use crate::config::MAX_CONFIG_BYTES;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WritePolicy {
    ReadOnly,
    CreateOnly,
    LocalWorkspace,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Observation {
    pub source: Option<String>,
    pub(crate) identity: Vec<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RawObservation {
    pub source: Option<Vec<u8>>,
    identity: Vec<u64>,
}

pub(crate) struct ConfigurationFile {
    directory: File,
    parent_path: PathBuf,
    name: String,
    policy: WritePolicy,
    serial: u64,
    pub(crate) backup: Option<String>,
}

impl ConfigurationFile {
    pub(crate) fn open(
        root: &Path,
        selection: Option<&Path>,
        policy: WritePolicy,
    ) -> Result<Self, ConfigurationError> {
        if !cfg!(all(target_os = "linux", target_arch = "x86_64")) || !root.is_absolute() {
            return Err(ConfigurationError::UnsupportedPath);
        }
        if root
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
        {
            return Err(ConfigurationError::UnsupportedPath);
        }
        let selection = selection.unwrap_or(Path::new(".fluzo"));
        let relative = if selection.is_absolute() {
            selection
                .strip_prefix(root)
                .map_err(|_| ConfigurationError::UnsupportedPath)?
        } else {
            selection
        };
        if relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(ConfigurationError::UnsupportedPath);
        }
        let name = relative
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| *name != "." && *name != ".." && name.len() <= 255)
            .ok_or(ConfigurationError::UnsupportedPath)?
            .to_owned();
        let parent_path = root.join(relative.parent().unwrap_or(Path::new("")));
        let mut directory = File::open("/").map_err(|_| ConfigurationError::Inaccessible)?;
        for component in parent_path.components() {
            if let Component::Normal(name) = component {
                #[cfg(target_os = "linux")]
                {
                    use std::os::fd::AsRawFd;
                    use std::os::unix::fs::OpenOptionsExt;
                    let path = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()))
                        .join(name);
                    directory = OpenOptions::new()
                        .read(true)
                        .custom_flags(0x20000 | 0x10000 | 0x800)
                        .open(path)
                        .map_err(|_| ConfigurationError::UnsupportedPath)?;
                }
            }
        }
        if !directory
            .metadata()
            .map_err(|_| ConfigurationError::Inaccessible)?
            .is_dir()
        {
            return Err(ConfigurationError::UnsupportedPath);
        }
        Ok(Self {
            directory,
            parent_path,
            name,
            policy,
            serial: 0,
            backup: None,
        })
    }

    fn anchored(&self, name: &str) -> PathBuf {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            PathBuf::from(format!(
                "/proc/self/fd/{}/{}",
                self.directory.as_raw_fd(),
                name
            ))
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.parent_path.join(name)
        }
    }

    fn verify_parent(&self) -> Result<(), ConfigurationError> {
        let current =
            fs::symlink_metadata(&self.parent_path).map_err(|_| ConfigurationError::Conflict)?;
        let held = self
            .directory
            .metadata()
            .map_err(|_| ConfigurationError::Inaccessible)?;
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt;
            if !current.is_dir() || current.dev() != held.dev() || current.ino() != held.ino() {
                return Err(ConfigurationError::Conflict);
            }
        }
        Ok(())
    }

    pub(crate) fn read(&self) -> Result<Observation, ConfigurationError> {
        let raw = self.read_raw()?;
        let source = raw.source.map(String::from_utf8).transpose().map_err(|_| {
            ConfigurationError::Invalid {
                code: fluzo_core::configuration::ConfigErrorCode::InvalidEncoding,
                key: "<document>".into(),
                span: None,
                errors: vec![],
            }
        })?;
        Ok(Observation {
            source,
            identity: raw.identity,
        })
    }

    pub(crate) fn read_raw(&self) -> Result<RawObservation, ConfigurationError> {
        self.verify_parent()?;
        let path = self.anchored(&self.name);
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            const NO_FOLLOW: i32 = 0x20000;
            const NONBLOCK: i32 = 0x800;
            options.custom_flags(NO_FOLLOW | NONBLOCK);
        }
        let file = match options.open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(RawObservation {
                    source: None,
                    identity: Vec::new(),
                });
            }
            Err(_) => return Err(ConfigurationError::Inaccessible),
        };
        let metadata = file
            .metadata()
            .map_err(|_| ConfigurationError::Inaccessible)?;
        if !metadata.is_file() {
            return Err(ConfigurationError::UnsupportedPath);
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() != 1 {
                return Err(ConfigurationError::UnsupportedPath);
            }
        }
        let mut source = Vec::new();
        (&file)
            .take(MAX_CONFIG_BYTES as u64 + 1)
            .read_to_end(&mut source)
            .map_err(|_| ConfigurationError::Inaccessible)?;
        if source.len() > MAX_CONFIG_BYTES {
            return Err(ConfigurationError::Capacity);
        }
        let after = file
            .metadata()
            .map_err(|_| ConfigurationError::Inaccessible)?;
        let current = fs::symlink_metadata(path).map_err(|_| ConfigurationError::Conflict)?;
        if identity(&metadata) != identity(&after) || identity(&after) != identity(&current) {
            return Err(ConfigurationError::Conflict);
        }
        Ok(RawObservation {
            source: Some(source),
            identity: identity(&after),
        })
    }

    /// Cheap existence-plus-identity check without reading contents, for polling
    /// whether something other than this writer touched the file.
    pub(crate) fn probe(&self) -> Result<Vec<u64>, ConfigurationError> {
        self.verify_parent()?;
        let path = self.anchored(&self.name);
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            const NO_FOLLOW: i32 = 0x20000;
            const NONBLOCK: i32 = 0x800;
            options.custom_flags(NO_FOLLOW | NONBLOCK);
        }
        let file = match options.open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(ConfigurationError::Inaccessible),
        };
        let metadata = file
            .metadata()
            .map_err(|_| ConfigurationError::Inaccessible)?;
        if !metadata.is_file() {
            return Err(ConfigurationError::UnsupportedPath);
        }
        Ok(identity(&metadata))
    }

    pub(crate) fn save(
        &mut self,
        expected: &Observation,
        source: &str,
    ) -> Result<Observation, ConfigurationError> {
        self.commit(
            &RawObservation {
                source: expected
                    .source
                    .as_ref()
                    .map(|text| text.as_bytes().to_vec()),
                identity: expected.identity.clone(),
            },
            source,
            false,
        )
    }

    pub(crate) fn prepare_setup(&self) -> Result<RawObservation, ConfigurationError> {
        let observed = self.read_raw()?;
        self.check_policy(observed.source.is_some())?;
        Ok(observed)
    }

    fn check_policy(&self, replacing: bool) -> Result<(), ConfigurationError> {
        if self.policy == WritePolicy::ReadOnly
            || (replacing && self.policy != WritePolicy::LocalWorkspace)
        {
            return Err(ConfigurationError::ReadOnly);
        }
        Ok(())
    }

    pub(crate) fn commit(
        &mut self,
        expected: &RawObservation,
        source: &str,
        backup: bool,
    ) -> Result<Observation, ConfigurationError> {
        self.check_policy(expected.source.is_some())?;
        if source.len() > MAX_CONFIG_BYTES {
            return Err(ConfigurationError::Capacity);
        }
        self.directory.try_lock().map_err(|error| match error {
            fs::TryLockError::WouldBlock => ConfigurationError::Busy,
            fs::TryLockError::Error(_) => ConfigurationError::Unavailable,
        })?;
        let result = self.save_locked(expected, source, backup);
        let unlocked = self.directory.unlock();
        if unlocked.is_err() && result.is_ok() {
            return Err(ConfigurationError::Uncertain);
        }
        result
    }

    fn save_locked(
        &mut self,
        expected: &RawObservation,
        source: &str,
        backup: bool,
    ) -> Result<Observation, ConfigurationError> {
        if &self.read_raw()? != expected {
            return Err(ConfigurationError::Conflict);
        }
        self.serial = self
            .serial
            .checked_add(1)
            .ok_or(ConfigurationError::Capacity)?;
        if backup && let Some(bytes) = &expected.source {
            let name = format!(".fluzo-backup-{}-{}", std::process::id(), self.serial);
            let path = self.anchored(&name);
            let mut options = OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(target_os = "linux")]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            self.fail_at(Fault::BackupWrite)?;
            let mut file = options
                .open(path)
                .map_err(|_| ConfigurationError::WriteFailed)?;
            self.backup = Some(name);
            file.write_all(bytes)
                .map_err(|_| ConfigurationError::WriteFailed)?;
            self.fail_at(Fault::BackupSync)?;
            file.sync_all()
                .map_err(|_| ConfigurationError::WriteFailed)?;
            use std::io::{Seek, SeekFrom};
            file.seek(SeekFrom::Start(0))
                .map_err(|_| ConfigurationError::WriteFailed)?;
            let mut verified = Vec::new();
            file.take(MAX_CONFIG_BYTES as u64 + 1)
                .read_to_end(&mut verified)
                .map_err(|_| ConfigurationError::WriteFailed)?;
            self.fail_at(Fault::BackupVerify)?;
            if verified != *bytes {
                return Err(ConfigurationError::WriteFailed);
            }
            self.fail_at(Fault::BackupDirectorySync)?;
            self.directory
                .sync_all()
                .map_err(|_| ConfigurationError::WriteFailed)?;
            if &self.read_raw()? != expected {
                return Err(ConfigurationError::Conflict);
            }
        }
        let temporary = self.anchored(&format!(
            ".fluzo-save-{}-{}",
            std::process::id(),
            self.serial
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|_| ConfigurationError::WriteFailed)?;
        let result = (|| {
            file.write_all(source.as_bytes())
                .map_err(|_| ConfigurationError::WriteFailed)?;
            self.fail_at(Fault::FileSync)?;
            file.sync_all()
                .map_err(|_| ConfigurationError::WriteFailed)?;
            if &self.read_raw()? != expected {
                return Err(ConfigurationError::Conflict);
            }
            self.fail_at(Fault::BeforeReplace)?;
            let target = self.anchored(&self.name);
            if expected.source.is_none() {
                fs::hard_link(&temporary, &target).map_err(|error| {
                    if error.kind() == std::io::ErrorKind::AlreadyExists {
                        ConfigurationError::Conflict
                    } else {
                        ConfigurationError::WriteFailed
                    }
                })?;
                fs::remove_file(&temporary).map_err(|_| ConfigurationError::Uncertain)?;
            } else {
                fs::rename(&temporary, &target).map_err(|_| ConfigurationError::WriteFailed)?;
            }
            self.fail_at(Fault::AfterReplace)?;
            self.fail_at(Fault::DirectorySync)?;
            self.directory
                .sync_all()
                .map_err(|_| ConfigurationError::Uncertain)?;
            let observed = self.read().map_err(|_| ConfigurationError::Uncertain)?;
            if observed.source.as_deref() != Some(source) {
                return Err(ConfigurationError::Uncertain);
            }
            Ok(observed)
        })();
        let _ = fs::remove_file(temporary);
        result
    }

    fn fail_at(&self, fault: Fault) -> Result<(), ConfigurationError> {
        #[cfg(test)]
        if FAILURE.with(|value| value.get()) == Some(fault) {
            return Err(
                if !matches!(fault, Fault::AfterReplace | Fault::DirectorySync) {
                    ConfigurationError::WriteFailed
                } else {
                    ConfigurationError::Uncertain
                },
            );
        }
        let _ = fault;
        Ok(())
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Fault {
    BackupWrite,
    BackupSync,
    BackupVerify,
    BackupDirectorySync,
    FileSync,
    BeforeReplace,
    AfterReplace,
    DirectorySync,
}

#[cfg(test)]
thread_local! { pub(crate) static FAILURE: std::cell::Cell<Option<Fault>> = const { std::cell::Cell::new(None) }; }

fn identity(metadata: &fs::Metadata) -> Vec<u64> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        vec![
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mtime() as u64,
            metadata.mtime_nsec() as u64,
            metadata.ctime() as u64,
            metadata.ctime_nsec() as u64,
            u64::from(metadata.mode()),
            metadata.nlink(),
        ]
    }
    #[cfg(not(target_os = "linux"))]
    {
        vec![metadata.len()]
    }
}
