#![feature(addr_parse_ascii)]

use std::net::SocketAddrV4;

use anyhow::Context;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use sha1::{Digest, Sha1};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use bittorrent_starter_rust::{
    Args, Commands, decode_bencoded, Handshake, Message, MessageFramer, MessageTag, Piece,
    Request, Torrent, TrackerRequest, TrackerResponse, urlencode,
};

const BLOCK_MAX: usize = 1 << 14;

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
        Commands::Handshake { torrent, peer } => {
            let f = std::fs::read(torrent).context("read torrent file")?;
            let t: Torrent = serde_bencode::from_bytes(&f).context("parse torrent file")?;

            let info_hash = t.info_hash();

            let peer = peer.parse::<SocketAddrV4>().context("parse peer addr")?;
            let mut peer = tokio::net::TcpStream::connect(peer)
                .await
                .context("connect to peer")?;
            let mut handshake = Handshake::new(info_hash, *b"00112233445566778899");
            {
                let handshake_bytes =
                    &mut handshake as *mut Handshake as *mut [u8; std::mem::size_of::<Handshake>()];
                // Safety: Handshake is a POD(Plain of Data) with repr(C)
                // which means any byte pattern is valid
                let handshake_bytes: &mut [u8; std::mem::size_of::<Handshake>()] =
                    unsafe { &mut *handshake_bytes };
                peer.write_all(handshake_bytes)
                    .await
                    .context("write handshake")?;
                peer.read_exact(handshake_bytes)
                    .await
                    .context("read handshake")?;
            }
            assert_eq!(handshake.length, 19);
            assert_eq!(&handshake.bittorrent, b"BitTorrent protocol");
            println!("Peer ID: {}", hex::encode(&handshake.peer_id));
        }
        Commands::DownloadPiece {
            output,
            torrent,
            piece,
        } => {
            let f = std::fs::read(torrent).context("read torrent file")?;
            let t: Torrent = serde_bencode::from_bytes(&f).context("parse torrent file")?;
            let length = t.length();
            let info_hash = t.info_hash();

            assert!(piece < t.info.pieces.0.len());

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
            let tracker_info: TrackerResponse =
                serde_bencode::from_bytes(&response).context("parse tracker response")?;

            let peer = &tracker_info.peers.0[0];

            let mut peer = tokio::net::TcpStream::connect(peer)
                .await
                .context("connect to peer")?;
            let mut handshake = Handshake::new(info_hash, *b"00112233445566778899");
            {
                let handshake_bytes = handshake.as_bytes_mut();
                peer.write_all(handshake_bytes)
                    .await
                    .context("write handshake")?;
                peer.read_exact(handshake_bytes)
                    .await
                    .context("read handshake")?;
            }
            println!("Peer ID: {}", hex::encode(&handshake.peer_id));

            let mut peer = tokio_util::codec::Framed::new(peer, MessageFramer);
            let bitfield = peer
                .next()
                .await
                .expect("peer always sends a bitfields")
                .context("peer message was invalid")?;
            assert_eq!(bitfield.tag, MessageTag::Bitfield);
            eprintln!("{:?}", bitfield.tag);

            peer.send(Message {
                tag: MessageTag::Interested,
                payload: Vec::new(),
            })
            .await
            .context("send interested message")?;

            let unchoke = peer
                .next()
                .await
                .expect("peer always sends an unchoke")
                .context("peer message was invalid")?;
            assert_eq!(unchoke.tag, MessageTag::Unchoke);
            assert!(unchoke.payload.is_empty());

            let piece_hash = t.info.pieces.0[piece];

            let piece_size = if piece == t.info.pieces.0.len() + 1 {
                length % t.info.plength
            } else {
                t.info.plength
            };
            let nblock = (piece_size + (BLOCK_MAX + 1)) / BLOCK_MAX;
            let mut all_blocks: Vec<u8> = Vec::with_capacity(piece_size);
            for block in 0..nblock {
                let block_size = if block == nblock - 1 {
                    piece_size % BLOCK_MAX
                } else {
                    BLOCK_MAX
                };
                let mut request =
                    Request::new(piece as u32, (block * BLOCK_MAX) as u32, block_size as u32);
                let request_bytes = request.as_bytes_mut();
                peer.send(Message {
                    tag: MessageTag::Request,
                    payload: request_bytes.to_vec(),
                })
                .await
                .with_context(|| format!("send request message for block {}", block))?;

                let piece = peer
                    .next()
                    .await
                    .expect("peer always sends an piece")
                    .context("peer message was invalid")?;
                assert_eq!(piece.tag, MessageTag::Piece);
                assert!(!piece.payload.is_empty());
                // trying to cast a thin pointer to a fat pointer
                let piece = &piece.payload[..] as *const [u8] as *const Piece;
                let piece = unsafe { &*piece };
                all_blocks.extend(piece.block());
            }
            let mut hasher = Sha1::new();
            hasher.update(&all_blocks);
            let hash = hasher.finalize();
            assert_eq!(piece_hash, hash.as_slice());

            tokio::fs::write(&output, all_blocks)
                .await
                .context("write out downloaded piece")?;
            println!("piece {:?} downloaded to {:?}.", piece, &output);
        }
    }

    Ok(())
}
