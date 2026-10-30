// A build script that stops just fails the build, with its message, so it may expect.
#![allow(clippy::expect_used)]

fn main() {
    slint_build::compile("ui/app.slint").expect("the Slint screens should compile");
}
