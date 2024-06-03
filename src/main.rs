use anyhow::Context;
use clap::Parser;

use bittorrent_starter_rust::{
    Args, Commands, decode_bencoded, Torrent, TrackerRequest, TrackerResponse, urlencode,
};

// Usage: your_bittorrent.sh decode "<encoded_value>"
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    match args.commands {
        Commands::Decode { value } => {
            let v = decode_bencoded(&value).0;
            println!("{v}");
        }
        Commands::Info { torrent } => {
            let f = std::fs::read(torrent).context("read torrent file")?;
            let t: Torrent = serde_bencode::from_bytes(&f).context("parse torrent file")?;
            // eprintln!("{t:?}");
            println!("Tracker URL: {}", t.announce);

            let length = t.length();
            println!("Length: {}", length);

            let info_hash = t.info_hash();
            println!("Info Hash: {}", hex::encode(info_hash));
            println!("Piece Length: {}", t.info.plength);
            println!("Piece Hashes:");
            for hash in t.info.pieces.0 {
                println!("{}", hex::encode(hash));
            }
        }
        Commands::Peers { torrent } => {
            let f = std::fs::read(torrent).context("read torrent file")?;
            let t: Torrent = serde_bencode::from_bytes(&f).context("parse torrent file")?;

            let length = t.length();
            let info_hash = t.info_hash();
            let request = TrackerRequest {
                peer_id: "00112233445566778899".to_string(),
                port: 6881,
                uploaded: 0,
                downloaded: 0,
                left: length,
                compact: 1,
            };

            let url_params =
                serde_urlencoded::to_string(&request).context("url-encode tracker parameters")?;

            let tracker_url = format!(
                "{}?{}&info_hash={}",
                t.announce,
                url_params,
                &urlencode(&info_hash)
            );

            let response = reqwest::get(tracker_url)
                .await
                .context("query tracker")?
                .bytes()
                .await
                .context("fetch tracker response")?;
            let response: TrackerResponse =
                serde_bencode::from_bytes(&response).context("parse tracker response")?;
            for peer in response.peers.0 {
                println!("{}:{}", peer.ip(), peer.port());
            }
        }
    }

    Ok(())
}
