use anyhow::Result;
use maelstrom_go_test::cli::ExtraCommandLineOptions;
use maelstrom_util::process::ExitCode;
use std::env;

pub fn main() -> Result<ExitCode> {
    maelstrom_test_runner::main(
        clap::command!(),
        "maelstrom/maelstrom-go-test",
        "MAELSTROM_GO_TEST",
        env::args(),
        |extra_options: &ExtraCommandLineOptions| extra_options.list,
        |_| Ok(".".into()),
        maelstrom_go_test::MAELSTROM_TEST_TOML,
        "",
        maelstrom_go_test::main,
    )
}
