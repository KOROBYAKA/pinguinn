use {
    clap::{Parser, crate_description, crate_name, crate_version},
    std::{net::SocketAddr, path::PathBuf, time::Duration},
};

#[derive(Parser, Debug, Clone)]
pub struct ClientCliParameters {
    #[clap(long, help = "Output file's name")]
    pub out_file: String,
}

pub fn build_cli_parameters() -> ClientCliParameters {
    ClientCliParameters::parse()
}
