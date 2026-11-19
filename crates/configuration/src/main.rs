use activator_configuration::{
    Compilable as _, Compilation, CompilationOutcome, TextualOutcome as _,
};

fn main() {
    let outcome = match Compilation::from_arguments(std::env::args_os().skip(1)) {
        Ok(compilation) => compilation.compile(),
        Err(refusal) => CompilationOutcome::CompileRefused(refusal),
    };
    println!("{}", outcome.text());
    if !outcome.succeeded() {
        std::process::exit(2);
    }
}
