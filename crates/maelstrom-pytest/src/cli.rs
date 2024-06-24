use clap::{command, Args};

#[derive(Args)]
#[command(next_help_heading = "Test Selection Options")]
pub struct ExtraCommandLineOptions {
    #[arg(
        long,
        short = 'i',
        value_name = "FILTER-EXPRESSION",
        default_value = "all",
        help = "Only include tests which match the given filter. Can be specified multiple times."
    )]
    pub include: Vec<String>,

    #[arg(
        long,
        short = 'x',
        value_name = "FILTER-EXPRESSION",
        help = "Only include tests which don't match the given filter. Can be specified multiple times."
    )]
    pub exclude: Vec<String>,

    #[arg(
        long = "list",
        help = "Instead of running tests, print the tests that would have been run."
    )]
    pub list: bool,

    #[arg(long = "pyargs", help = "This argument is passed through to pytest.")]
    pub pyargs: Option<String>,

    #[arg(long, hide(true), help = "Only used for testing purposes")]
    pub client_bg_proc: bool,
}
