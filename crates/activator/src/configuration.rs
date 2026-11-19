//! The version-0 configuration file. Only desired configuration is persisted;
//! live generations and activation outcomes are read or returned, never stored.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use meta_signal_activator_nexus::Configuration;
use signal::{ByteViewable as _, Restorable as _, Signal, Signalizable as _};

use crate::activation::{Activator, Configurable as _};

pub struct ConfigurationFile {
    path: PathBuf,
}

pub trait ConfigurationStorage: Sized {
    fn at_path(path: PathBuf) -> Self;
    fn path(&self) -> &Path;
    fn read(&self) -> std::io::Result<Configuration>;
    fn replace(&self, configuration: &Configuration) -> std::io::Result<()>;
}

impl ConfigurationStorage for ConfigurationFile {
    fn at_path(path: PathBuf) -> Self {
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn read(&self) -> std::io::Result<Configuration> {
        let bytes = std::fs::read(&self.path)?;
        let configuration = Signal::<Configuration>::from(bytes)
            .restore()
            .map_err(std::io::Error::other)?;
        Activator::configured(configuration.clone()).map_err(std::io::Error::other)?;
        Ok(configuration)
    }

    fn replace(&self, configuration: &Configuration) -> std::io::Result<()> {
        Activator::configured(configuration.clone()).map_err(std::io::Error::other)?;
        let parent = self.path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "configuration needs a parent directory",
            )
        })?;
        let signal = configuration.signalize().map_err(std::io::Error::other)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(signal.bytes())?;
        temporary.as_file().sync_all()?;
        temporary.persist(&self.path).map_err(|error| error.error)?;
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    }
}
