//! `formatlab`: Format Lab from a terminal. `formatlab help` lists what it does.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = mo_formatlab::command::run(&args, &mut std::io::stdout().lock(), &mut std::io::stderr().lock());
    std::process::exit(code);
}
