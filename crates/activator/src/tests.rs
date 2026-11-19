use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};

use meta_signal_activator_nexus as meta;
use signal_activator_nexus::{ActivateRefusal, Generation, LiveRefusal, ProcessOutcome};

use crate::activation::{Activatable as _, Activator, Configurable as _};

struct Fixture {
    directory: tempfile::TempDir,
    configuration: meta::Configuration,
    generation: Generation,
}

trait Constructible: Sized {
    fn fixture(activation: &str) -> Self;
    fn activator(&self) -> Activator;
    fn profile(&self) -> &Path;
}

impl Constructible for Fixture {
    fn fixture(activation: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path().join("store");
        let generation = store.join("realized-generation");
        let runtime = directory.path().join("runtime");
        std::fs::create_dir_all(&generation).unwrap();
        std::fs::create_dir(&runtime).unwrap();
        let shell = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|directory| directory.join("sh"))
            .find(|path| path.is_file())
            .unwrap();
        let script = generation.join("activate");
        std::fs::write(&script, format!("#!{}\n{activation}\n", shell.display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let literal = |text: &str| meta::Argument::Literal(meta::Literal(text.to_owned()));
        let configuration = meta::Configuration {
            ordinary_socket_path: meta::OrdinarySocketPath(
                runtime.join("ordinary.sock").display().to_string(),
            ),
            meta_socket_path: meta::MetaSocketPath(runtime.join("meta.sock").display().to_string()),
            runtime_directory: meta::RuntimeDirectory(runtime.display().to_string()),
            store_directory: meta::StoreDirectory(store.display().to_string()),
            profile_path: meta::ProfilePath(directory.path().join("profile").display().to_string()),
            validation_command: meta::ValidationCommand {
                executable: meta::Executable::ProgramPath(meta::ProgramPath(
                    shell.display().to_string(),
                )),
                argument_vector: vec![
                    literal("-c"),
                    literal("test -d \"$1\""),
                    literal("validate"),
                    meta::Argument::Generation,
                ],
            },
            activation_command: meta::ActivationCommand {
                executable: meta::Executable::GenerationRelativePath("activate".into()),
                argument_vector: vec![meta::Argument::Profile],
            },
            profile_command: meta::ProfileCommand {
                executable: meta::Executable::ProgramPath(meta::ProgramPath(
                    shell.display().to_string(),
                )),
                argument_vector: vec![
                    literal("-c"),
                    literal("ln -sfn -- \"$1\" \"$2\""),
                    literal("publish"),
                    meta::Argument::Generation,
                    meta::Argument::Profile,
                ],
            },
        };
        Self {
            directory,
            configuration,
            generation: Generation(generation.display().to_string()),
        }
    }

    fn activator(&self) -> Activator {
        Activator::configured(self.configuration.clone()).unwrap()
    }

    fn profile(&self) -> &Path {
        Path::new(&self.configuration.profile_path.0)
    }
}

#[tokio::test]
async fn publishes_only_after_activation_succeeds() {
    let fixture = Fixture::fixture("test ! -e \"$1\" || exit 19");
    let activator = fixture.activator();
    assert_eq!(
        activator
            .activate(fixture.generation.clone())
            .await
            .unwrap(),
        fixture.generation
    );
    assert_eq!(activator.live().await.unwrap(), fixture.generation);
}

#[tokio::test]
async fn failed_activation_preserves_existing_profile_and_evidence() {
    let fixture = Fixture::fixture("printf 'activation failure' >&2; exit 23");
    let previous = PathBuf::from(&fixture.configuration.store_directory.0).join("previous");
    std::fs::create_dir(&previous).unwrap();
    symlink(&previous, fixture.profile()).unwrap();
    let activator = fixture.activator();
    match activator.activate(fixture.generation.clone()).await {
        Err(ActivateRefusal::Failed(evidence)) => {
            assert_eq!(evidence.process_outcome, ProcessOutcome::Exited(23));
            assert_eq!(evidence.string, "activation failure");
            assert_eq!(
                evidence.command.program_path.0,
                format!("{}/activate", fixture.generation.0)
            );
        }
        outcome => panic!("expected failed activation, got {outcome:?}"),
    }
    assert_eq!(std::fs::canonicalize(fixture.profile()).unwrap(), previous);
}

#[tokio::test]
async fn live_reads_external_profile_changes_without_remembering_a_receipt() {
    let fixture = Fixture::fixture("exit 0");
    let activator = fixture.activator();
    assert_eq!(activator.live().await, Err(LiveRefusal::NoLiveGeneration));
    symlink(&fixture.generation.0, fixture.profile()).unwrap();
    assert_eq!(activator.live().await.unwrap(), fixture.generation);
    let external = PathBuf::from(&fixture.configuration.store_directory.0).join("external");
    std::fs::create_dir(&external).unwrap();
    std::fs::remove_file(fixture.profile()).unwrap();
    symlink(&external, fixture.profile()).unwrap();
    assert_eq!(
        activator.live().await.unwrap().0,
        external.display().to_string()
    );
}

#[tokio::test]
async fn refuses_unrealized_and_outside_store_generations() {
    let fixture = Fixture::fixture("exit 0");
    let activator = fixture.activator();
    for path in [
        PathBuf::from(&fixture.configuration.store_directory.0).join("missing"),
        fixture.directory.path().to_path_buf(),
        PathBuf::from(&fixture.configuration.store_directory.0).join("../outside"),
    ] {
        let generation = Generation(path.display().to_string());
        assert_eq!(
            activator.activate(generation.clone()).await,
            Err(ActivateRefusal::NotRealized(generation))
        );
    }
    assert!(!fixture.profile().exists());
}

#[tokio::test]
async fn reports_profile_publication_failure_after_successful_activation() {
    let mut fixture = Fixture::fixture("exit 0");
    fixture.configuration.profile_command.argument_vector = vec![
        meta::Argument::Literal(meta::Literal("-c".into())),
        meta::Argument::Literal(meta::Literal(
            "printf 'profile refused' >&2; exit 29".into(),
        )),
    ];
    match fixture
        .activator()
        .activate(fixture.generation.clone())
        .await
    {
        Err(ActivateRefusal::Failed(evidence)) => {
            assert_eq!(evidence.process_outcome, ProcessOutcome::Exited(29));
            assert_eq!(evidence.string, "profile refused");
        }
        outcome => panic!("expected failed publication, got {outcome:?}"),
    }
    assert!(!fixture.profile().exists());
}

#[tokio::test]
async fn successful_noop_publisher_does_not_claim_activation() {
    let mut fixture = Fixture::fixture("exit 0");
    fixture.configuration.profile_command.argument_vector = vec![
        meta::Argument::Literal(meta::Literal("-c".into())),
        meta::Argument::Literal(meta::Literal("exit 0".into())),
    ];
    assert_eq!(
        fixture
            .activator()
            .activate(fixture.generation.clone())
            .await,
        Err(ActivateRefusal::ProfileUnchanged(fixture.generation))
    );
}

#[test]
fn rejects_sockets_outside_the_session_runtime_directory() {
    let mut fixture = Fixture::fixture("exit 0");
    fixture.configuration.ordinary_socket_path.0 = fixture
        .directory
        .path()
        .join("outside.sock")
        .display()
        .to_string();
    assert!(Activator::configured(fixture.configuration).is_err());
}

#[test]
fn configuration_replacement_preserves_only_the_desired_configuration() {
    use crate::configuration::{ConfigurationFile, ConfigurationStorage as _};
    let mut fixture = Fixture::fixture("exit 0");
    let path = fixture.directory.path().join("configuration.signal");
    let file = ConfigurationFile::at_path(path);
    file.replace(&fixture.configuration).unwrap();
    assert_eq!(file.read().unwrap(), fixture.configuration);
    fixture.configuration.profile_path.0 = fixture
        .directory
        .path()
        .join("changed-profile")
        .display()
        .to_string();
    file.replace(&fixture.configuration).unwrap();
    assert_eq!(file.read().unwrap(), fixture.configuration);
    assert!(!fixture.profile().exists());
}

#[test]
fn invalid_configuration_replacement_preserves_the_previous_file() {
    use crate::configuration::{ConfigurationFile, ConfigurationStorage as _};
    let mut fixture = Fixture::fixture("exit 0");
    let file = ConfigurationFile::at_path(fixture.directory.path().join("configuration.signal"));
    file.replace(&fixture.configuration).unwrap();
    let original = std::fs::read(file.path()).unwrap();
    fixture.configuration.ordinary_socket_path.0 = "relative.sock".into();
    assert!(file.replace(&fixture.configuration).is_err());
    assert_eq!(std::fs::read(file.path()).unwrap(), original);
}

#[tokio::test]
async fn ordinary_signal_answers_activation_and_profile_observation() {
    use crate::ordinary::Answerable as _;
    use signal_activator_nexus::{Query, Response};
    let fixture = Fixture::fixture("exit 0");
    let activator = fixture.activator();
    assert_eq!(
        activator.answer(Query::Live).await,
        Response::LiveRefused(LiveRefusal::NoLiveGeneration)
    );
    assert_eq!(
        activator
            .answer(Query::Activate(fixture.generation.clone()))
            .await,
        Response::Activated(fixture.generation.clone())
    );
    assert_eq!(
        activator.answer(Query::Live).await,
        Response::Live(fixture.generation)
    );
}
