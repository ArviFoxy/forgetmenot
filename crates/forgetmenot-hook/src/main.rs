//! Thin wrapper: the work is in the library so it can be tested without a
//! process.

use std::io::{stderr, stdin, stdout};
use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let code = forgetmenot_hook::run(
        &arguments,
        &mut stdin().lock(),
        &mut stdout().lock(),
        &mut stderr().lock(),
    );
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
