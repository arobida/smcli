mod cli;
mod context;
mod highlight;
mod initialize;
mod menu;
mod picker;
mod shell_integration;
mod shell_quoting;

fn main() {
    if let Err(error) = cli::run() {
        eprintln!("smcli: {error:#}");
        std::process::exit(1);
    }
}
