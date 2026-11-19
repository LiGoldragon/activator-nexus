//! The component-owned configuration compiler: one Datom, one startup archive.
//! Its output is exactly the Configuration restored by the session service.

use std::ffi::OsString;
use std::path::PathBuf;

use activator::configuration::{ConfigurationFile, ConfigurationStorage as _};
use datom_codec::{Actualizing as _, Budget, Datomizable as _, Potential};
use protos::{Protosizable as _, Textualizable as _};

include!(concat!(env!("OUT_DIR"), "/entry.rs"));

pub trait Compilable: Sized {
    fn from_arguments(
        arguments: impl IntoIterator<Item = OsString>,
    ) -> Result<Self, CompilationRefusal>;
    fn compile(&self) -> CompilationOutcome;
}

pub trait TextualOutcome {
    fn text(&self) -> String;
    fn succeeded(&self) -> bool;
}

impl TextualOutcome for CompilationOutcome {
    fn text(&self) -> String {
        self.datomize(Vec::new()).protosize().textualize()
    }

    fn succeeded(&self) -> bool {
        matches!(self, Self::Compiled(_))
    }
}

impl Compilable for Compilation {
    fn from_arguments(
        arguments: impl IntoIterator<Item = OsString>,
    ) -> Result<Self, CompilationRefusal> {
        let mut arguments = arguments.into_iter();
        let text = arguments.next().ok_or(CompilationRefusal::ArgumentCount)?;
        if arguments.next().is_some() {
            return Err(CompilationRefusal::ArgumentCount);
        }
        let text = text
            .into_string()
            .map_err(|_| CompilationRefusal::NonUnicode)?;
        // Each structural node consumes at least one input byte. The compiler's
        // one reading act is bounded by the size of the Datom it was given.
        let remaining = i64::try_from(text.len())
            .map_err(|error| CompilationRefusal::RequestText(error.to_string()))?;
        let reader = protos::ReaderBudget::try_from(text.len())
            .map_err(|error| CompilationRefusal::RequestText(format!("{error:?}")))?;
        let mut budget = Budget {
            remaining,
            reader,
            depth: 0,
            maximum_depth: remaining,
        };
        Potential::<Self>::from(text)
            .actualize(&mut budget)
            .map_err(|error| CompilationRefusal::RequestText(format!("{error:?}")))
    }

    fn compile(&self) -> CompilationOutcome {
        let path = PathBuf::from(&self.configuration_file_path.0);
        if !path.is_absolute() {
            return CompilationOutcome::CompileRefused(CompilationRefusal::InvalidPath(
                self.configuration_file_path.0.clone(),
            ));
        }
        let file = ConfigurationFile::at_path(path);
        match file.replace(&self.configuration) {
            Ok(()) => CompilationOutcome::Compiled(self.configuration_file_path.clone()),
            Err(error) => CompilationOutcome::CompileRefused(CompilationRefusal::Publication(
                error.to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_missing_or_multiple_inline_arguments() {
        assert_eq!(
            Compilation::from_arguments(Vec::<OsString>::new()),
            Err(CompilationRefusal::ArgumentCount)
        );
        assert_eq!(
            Compilation::from_arguments([OsString::from("{}"), OsString::from("{}")]),
            Err(CompilationRefusal::ArgumentCount)
        );
    }

    #[test]
    fn refuses_a_request_of_another_type() {
        assert!(matches!(
            Compilation::from_arguments([OsString::from("Activate./nix/store/example")]),
            Err(CompilationRefusal::RequestText(_))
        ));
    }

    #[test]
    fn compiles_the_exact_configuration_type_the_service_restores() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().display();
        let text = format!(
            "{{ {root}/configuration.signal {{ {root}/runtime/ordinary.sock \
             {root}/runtime/meta.sock {root}/runtime {root}/store {root}/profile \
             {{ ProgramPath./bin/true [] }} \
             {{ GenerationRelativePath.activate [] }} \
             {{ ProgramPath./bin/true [] }} }} }}"
        );
        let compilation = Compilation::from_arguments([OsString::from(text)]).unwrap();
        assert_eq!(
            compilation.compile(),
            CompilationOutcome::Compiled(compilation.configuration_file_path.clone())
        );
        let file =
            ConfigurationFile::at_path(PathBuf::from(&compilation.configuration_file_path.0));
        assert_eq!(file.read().unwrap(), compilation.configuration);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
