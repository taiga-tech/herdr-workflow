use herdr_workflow::{cli, commands};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // TaskRunner(Coordinatorが内部で起動する。ADR-0002)。
    if args.first().map(String::as_str) == Some("runner") {
        std::process::exit(herdr_workflow::runtime::runner::main());
    }
    // `herdrAction`のjob(Q08)。
    if args.first().map(String::as_str) == Some("herdr-action") {
        std::process::exit(herdr_workflow::herdr_action::main(&args[1..]));
    }
    if args
        .first()
        .is_some_and(|subcommand| commands::handles(subcommand))
    {
        std::process::exit(commands::main(&args));
    }
    let (code, output) = match cli::parse_args(&args) {
        Ok(command) => cli::execute_from_environment(&command),
        Err(err) => cli::parse_error_output(&args, &err),
    };
    println!("{output}");
    std::process::exit(code);
}
