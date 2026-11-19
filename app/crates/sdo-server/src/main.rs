mod asset_http;
mod browser;

use anyhow::{Context, Result, bail, ensure};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut game = None;
    let mut assets = None;
    let mut http = None;
    let mut web = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--web" => web = true,
            "--assets" => {
                assets = Some(PathBuf::from(
                    args.next().context("--assets needs a directory")?,
                ))
            }
            "--asset-listen" => {
                http = Some(args.next().context("--asset-listen needs an address")?)
            }
            "-h" | "--help" => {
                println!(
                    "sdo-server [GAME_ADDRESS] [--assets DIRECTORY] [--web] [--asset-listen ADDRESS]\nGame TCP defaults to 0.0.0.0:32100. Optional web player (/play/) and asset HTTP listener defaults to 0.0.0.0:32101."
                );
                return Ok(());
            }
            _ if arg.starts_with("--assets=") => assets = Some(PathBuf::from(&arg[9..])),
            _ if !arg.starts_with('-') && game.is_none() => game = Some(arg),
            _ => bail!("Unexpected server argument {arg:?}; use --help"),
        }
    }
    ensure!(
        assets.is_some() || web || http.is_none(),
        "--asset-listen requires --assets or --web"
    );
    let game = game.as_deref().unwrap_or("0.0.0.0:32100");
    if assets.is_some() || web {
        let lobby = sdo_multiplayer::Server::bind(game)?;
        println!(
            "SDO dedicated server listening on {} (protocol {}, multiple rooms)",
            lobby.address,
            sdo_net::PROTOCOL
        );
        asset_http::serve(
            assets.as_deref(),
            http.as_deref().unwrap_or("0.0.0.0:32101"),
            lobby.address,
            web,
        )
    } else {
        sdo_multiplayer::Server::run(game)
    }
}
