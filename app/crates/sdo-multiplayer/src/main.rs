fn main() -> anyhow::Result<()> {
    let address = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:32100".into());
    sdo_multiplayer::Server::run(&address)
}
