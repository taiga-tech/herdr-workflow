use herdr_workflow::cli;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (code, output) = match cli::parse_args(&args) {
        Ok(command) => cli::execute(&command),
        Err(err) => (
            2,
            format!(
                "{{\"ok\":false,\"error\":{{\"stage\":\"cli\",\"message\":{:?}}}}}",
                err.to_string()
            ),
        ),
    };
    println!("{output}");
    std::process::exit(code);
}
