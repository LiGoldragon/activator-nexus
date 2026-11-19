//! Target-local activation effects. The serving actor owns each call to completion;
//! a client's disconnect does not cancel the operation or release its exclusion.

use std::path::{Component, Path, PathBuf};

use meta_signal_activator_nexus::{
    ActivationCommand, Argument, Configuration, Executable, ProfileCommand, ValidationCommand,
};
use signal_activator_nexus::{
    ActivateRefusal, Command, Evidence, Generation, LiveRefusal, ProcessOutcome, StorePath,
};
use tokio::sync::Mutex;

/// The configuration and the single in-flight operation in this process.
/// The profile, read afresh, is the only source of live-generation information.
pub struct Activator {
    pub configuration: Configuration,
    exclusion: Mutex<()>,
}

pub trait Configurable: Sized {
    fn configured(configuration: Configuration) -> Result<Self, String>;
}

pub trait Activatable {
    fn activate(
        &self,
        generation: Generation,
    ) -> impl Future<Output = Result<Generation, ActivateRefusal>> + Send;
    fn live(&self) -> impl Future<Output = Result<Generation, LiveRefusal>> + Send;
}

/// Expanding configured command data into an argv, without a shell or a login.
pub trait Expandable {
    fn executable(&self) -> &Executable;
    fn arguments(&self) -> &[Argument];

    fn expand(&self, generation: &Generation, profile: &Path) -> Result<Command, String> {
        let executable = match self.executable() {
            Executable::ProgramPath(path) => PathBuf::from(&path.0),
            Executable::GenerationRelativePath(path) => {
                let relative = Path::new(path);
                if relative.as_os_str().is_empty()
                    || relative
                        .components()
                        .any(|part| !matches!(part, Component::Normal(_)))
                {
                    return Err("activation executable must be relative to its generation".into());
                }
                Path::new(&generation.0).join(relative)
            }
        };
        if !executable.is_absolute() {
            return Err("configured executable must be absolute".into());
        }
        let profile = profile
            .to_str()
            .ok_or_else(|| "configured profile path must be UTF-8".to_string())?;
        Ok(Command {
            program_path: signal_activator_nexus::ProgramPath(
                executable.to_string_lossy().into_owned(),
            ),
            argument_vector: self
                .arguments()
                .iter()
                .map(|argument| {
                    signal_activator_nexus::Argument(match argument {
                        Argument::Literal(literal) => literal.0.clone(),
                        Argument::Generation => generation.0.clone(),
                        Argument::Profile => profile.to_string(),
                    })
                })
                .collect(),
        })
    }
}

impl Expandable for ValidationCommand {
    fn executable(&self) -> &Executable {
        &self.executable
    }
    fn arguments(&self) -> &[Argument] {
        &self.argument_vector
    }
}

impl Expandable for ActivationCommand {
    fn executable(&self) -> &Executable {
        &self.executable
    }
    fn arguments(&self) -> &[Argument] {
        &self.argument_vector
    }
}

impl Expandable for ProfileCommand {
    fn executable(&self) -> &Executable {
        &self.executable
    }
    fn arguments(&self) -> &[Argument] {
        &self.argument_vector
    }
}

/// A subprocess's typed termination and output, including failure to start.
pub trait ExecutableCommand {
    fn execute(&self) -> impl Future<Output = Result<(), Evidence>> + Send;
}

impl ExecutableCommand for Command {
    async fn execute(&self) -> Result<(), Evidence> {
        let result = tokio::process::Command::new(&self.program_path.0)
            .args(self.argument_vector.iter().map(|argument| &argument.0))
            .kill_on_drop(true)
            .output()
            .await;
        match result {
            Ok(output) if output.status.success() => Ok(()),
            Ok(output) => {
                use std::os::unix::process::ExitStatusExt as _;
                let process_outcome = match output.status.code() {
                    Some(code) => ProcessOutcome::Exited(i64::from(code)),
                    None => match output.status.signal() {
                        Some(signal) => ProcessOutcome::Signalled(i64::from(signal)),
                        None => ProcessOutcome::NotStarted(output.status.to_string()),
                    },
                };
                let mut bytes = output.stdout;
                bytes.extend(output.stderr);
                Err(Evidence {
                    command: self.clone(),
                    process_outcome,
                    string: String::from_utf8_lossy(&bytes).into_owned(),
                })
            }
            Err(error) => Err(Evidence {
                command: self.clone(),
                process_outcome: ProcessOutcome::NotStarted(error.to_string()),
                string: error.to_string(),
            }),
        }
    }
}

/// Validating and observing filesystem facts owned by the local target.
trait Locatable {
    fn generation_path(&self, generation: &Generation) -> Result<PathBuf, String>;
    fn profile_path(&self) -> &Path;
    fn read_live(&self) -> impl Future<Output = Result<Generation, LiveRefusal>> + Send;
}

impl Locatable for Activator {
    fn generation_path(&self, generation: &Generation) -> Result<PathBuf, String> {
        let path = Path::new(&generation.0);
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
            || path.parent() != Some(Path::new(&self.configuration.store_directory.0))
            || path.file_name().is_none()
        {
            return Err("generation must name one output in the configured store".into());
        }
        Ok(path.to_path_buf())
    }

    fn profile_path(&self) -> &Path {
        Path::new(&self.configuration.profile_path.0)
    }

    async fn read_live(&self) -> Result<Generation, LiveRefusal> {
        let path = match tokio::fs::canonicalize(self.profile_path()).await {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(LiveRefusal::NoLiveGeneration);
            }
            Err(error) => return Err(LiveRefusal::Unreadable(error.to_string())),
        };
        let path = path
            .to_str()
            .ok_or_else(|| LiveRefusal::Unreadable("live profile target must be UTF-8".into()))?;
        let generation = Generation(path.to_owned());
        self.generation_path(&generation)
            .map_err(|_| LiveRefusal::NotRealized(generation.clone()))?;
        Ok(generation)
    }
}

impl Configurable for Activator {
    fn configured(configuration: Configuration) -> Result<Self, String> {
        for path in [
            &configuration.runtime_directory.0,
            &configuration.store_directory.0,
            &configuration.profile_path.0,
            &configuration.ordinary_socket_path.0,
            &configuration.meta_socket_path.0,
        ] {
            if !Path::new(path).is_absolute() {
                return Err("configured paths must be absolute".into());
            }
        }
        let runtime = Path::new(&configuration.runtime_directory.0);
        for socket in [
            &configuration.ordinary_socket_path.0,
            &configuration.meta_socket_path.0,
        ] {
            if !Path::new(socket).starts_with(runtime) {
                return Err(
                    "activator sockets must be inside its session runtime directory".into(),
                );
            }
        }
        if configuration.ordinary_socket_path.0 == configuration.meta_socket_path.0 {
            return Err("ordinary and meta sockets must be distinct".into());
        }
        Ok(Self {
            configuration,
            exclusion: Mutex::new(()),
        })
    }
}

impl Activatable for Activator {
    async fn activate(&self, generation: Generation) -> Result<Generation, ActivateRefusal> {
        let _operation = self
            .exclusion
            .try_lock()
            .map_err(|_| ActivateRefusal::Collision(StorePath(generation.0.clone())))?;
        self.generation_path(&generation)
            .map_err(|_| ActivateRefusal::NotRealized(generation.clone()))?;
        let profile = self.profile_path();
        let validation = self
            .configuration
            .validation_command
            .expand(&generation, profile)
            .map_err(|_| ActivateRefusal::NotRealized(generation.clone()))?;
        let activation = self
            .configuration
            .activation_command
            .expand(&generation, profile)
            .map_err(|_| ActivateRefusal::NotRealized(generation.clone()))?;
        let publication = self
            .configuration
            .profile_command
            .expand(&generation, profile)
            .map_err(|_| ActivateRefusal::ProfileUnchanged(generation.clone()))?;
        validation
            .execute()
            .await
            .map_err(|_| ActivateRefusal::NotRealized(generation.clone()))?;
        activation
            .execute()
            .await
            .map_err(ActivateRefusal::Failed)?;
        publication
            .execute()
            .await
            .map_err(ActivateRefusal::Failed)?;
        match self.read_live().await {
            Ok(live) if live.0 == generation.0 => Ok(live),
            _ => Err(ActivateRefusal::ProfileUnchanged(generation)),
        }
    }

    async fn live(&self) -> Result<Generation, LiveRefusal> {
        let _operation = self.exclusion.lock().await;
        self.read_live().await
    }
}
