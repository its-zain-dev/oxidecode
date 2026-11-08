fn main() {
    if let Err(error) = oxidecode_cli::run() {
        eprintln!("{}", format_error(&error));
        std::process::exit(1);
    }
}

fn format_error(error: &anyhow::Error) -> String {
    let mut out = format!("error: {error}");
    for cause in error.chain().skip(1) {
        out.push_str(&format!("\n  cause: {cause}"));
    }
    out
}
