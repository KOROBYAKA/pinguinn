use clap::Parser;

// My Clap parser
#[derive(Parser)]
pub struct CliArgs {
    #[arg(
        long,
        help = "Input file in Solana gossip format",
        default_value = "gossip.log"
    )]
    pub input_file: String,

    #[arg(long, help = "Results output file", default_value = "ping.csv")]
    pub output_file: String,

    #[arg(long, help = "Every node connection retries", default_value = "3")]
    pub retries: u16,

    #[arg(
        long,
        help = "Amount of concurrent connections(recommended max 256)",
        default_value = "64"
    )]
    pub concurrent_connections: usize,

    #[arg(
        long,
        help = "Connection establishment timeout (milliseconds)",
        default_value = "30000"
    )]
    pub connection_timeout: u64,

    #[arg(
        long,
        help = "Delay measurement mode:\n 1:ICMP(default ping)\n 2:QUIC(handshake RTT)\n 3:Both",
        default_value = "1"
    )]
    pub mode: u16,
}

pub fn parse_args() -> CliArgs {
    CliArgs::parse()
}
