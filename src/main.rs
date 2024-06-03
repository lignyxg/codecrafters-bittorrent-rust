use anyhow::Context;
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use bittorrent_starter_rust::{Args, Commands, Hashes};

// Available if you need it!
// use serde_bencode

/// Metainfo files (also known as .torrent files) are bencoded dictionaries with the following keys:
#[derive(Debug, Clone, Deserialize, Serialize)]
struct Torrent {
    /// The URL of the tracker
    announce: String,
    /// This maps to a dictionary, with keys described below.
    info: Info,
}

#[derive(Debug, Deserialize, Clone, Serialize)]
struct Info {
    /// The name key maps to a UTF-8 encoded string which is the suggested name to
    /// save the file (or directory) as. It is purely advisory.
    /// In the single file case, the name key is the name of a file,
    /// in the muliple file case, it's the name of a directory.
    name: String,
    /// The number of bytes in each piece the file is split into.
    /// For the purposes of transfer, files are split into fixed-size pieces which are
    /// all the same length except for possibly the last one which may be truncated.
    /// piece length is almost always a power of two,
    /// most commonly 2^18 = 256 K (BitTorrent prior to version 3.2 uses 2^20 = 1 M as default).
    #[serde(rename = "piece length")]
    plength: usize,
    /// pieces maps to a string whose length is a multiple of 20.
    /// It is to be subdivided into strings of length 20,
    /// each of entry of `pieces` is the SHA1 hash of the piece at the corresponding index.
    pieces: Hashes,
    /// There is also a key length or a key files, but not both or neither.

    #[serde(flatten)]
    keys: Keys,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
enum Keys {
    /// If length is present then the download represents a single file,
    /// In the single file case, length maps to the length of the file in bytes.
    SingleFile { length: usize },

    /// otherwise it represents a set of files which go in a directory structure.
    /// the multi-file case is treated as only having a single file by concatenating
    /// the files in the order they appear in the files list. The files list is the value
    /// files maps to, and is a list of dictionaries containing the following keys:
    MultiFile { files: Vec<File> },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct File {
    /// The length of the file, in bytes.
    length: usize,
    /// A list of UTF-8 encoded strings corresponding to subdirectory names,
    /// the last of which is the actual file name (a zero length list is an error case).
    path: Vec<String>,
}

fn decode_bencoded(encoded_value: &str) -> (serde_json::Value, &str) {
    match encoded_value.chars().next() {
        Some('i') => {
            eprintln!("i: {}", encoded_value);
            if let Some((pre, rest)) = encoded_value.split_once('e') {
                let mut pre = pre.to_string();
                pre.push('e');
                let v = serde_bencode::from_str::<serde_json::Number>(&pre).unwrap();
                return (v.into(), rest);
            }
        }
        Some('l') => {
            let mut rest = encoded_value.split_at(1).1;
            let mut values = Vec::new();
            while !rest.is_empty() && !rest.starts_with('e') {
                let (v, remainder) = decode_bencoded(rest);
                values.push(v);
                rest = remainder;
                eprintln!("rest: {rest}");
            }
            return (values.into(), &rest[1..]);
        }
        Some('d') => {
            let mut dict = serde_json::Map::new();
            let mut rest = encoded_value.split_at(1).1;
            while !rest.is_empty() && !rest.starts_with('e') {
                let (k, remainder) = decode_bencoded(rest);
                let k = match k {
                    Value::String(k) => k,
                    k => panic!("dict keys should be string, not {k:?}"),
                };
                let (v, remainder) = decode_bencoded(remainder);
                dict.insert(k, v);
                rest = remainder;
            }
            return (dict.into(), &rest[1..]);
        }
        Some('0'..='9') => {
            let n = encoded_value.split_at(1).0.parse::<usize>().unwrap();
            let (pre, rest) = encoded_value.split_at(n + 2);
            let v = serde_bencode::from_str::<String>(pre).unwrap();
            return (v.into(), rest);
        }
        _ => {}
    }
    panic!("Unhandled encoded value: {}", encoded_value);
}

#[allow(dead_code)]
fn decode_bencoded_value(encoded_value: &str) -> (serde_json::Value, &str) {
    // If encoded_value starts with a digit, it's a number
    match encoded_value.chars().next() {
        Some('i') => {
            if let Some((n, rest)) =
                encoded_value
                    .split_at(1)
                    .1
                    .split_once('e')
                    .and_then(|(digits, rest)| {
                        let n = digits.parse::<i64>().ok()?;
                        Some((n, rest))
                    })
            {
                return (n.into(), rest);
            }
        }
        Some('l') => {
            let mut values = Vec::new();
            let mut rest = encoded_value.split_at(1).1;
            while !rest.is_empty() && !rest.starts_with('e') {
                let (v, remainder) = decode_bencoded_value(rest);
                rest = remainder;
                values.push(v);
            }

            return (values.into(), &rest[1..]);
        }
        Some('d') => {
            let mut dict = serde_json::Map::new();
            let mut rest = encoded_value.split_at(1).1;
            while !rest.is_empty() && !rest.starts_with('e') {
                let (k, remainder) = decode_bencoded_value(rest);
                let k = match k {
                    Value::String(k) => k,
                    k => panic!("dict keys should be string, not {k:?}"),
                };
                let (v, remainder) = decode_bencoded_value(remainder);
                rest = remainder;
                dict.insert(k, v);
            }

            return (dict.into(), &rest[1..]);
        }
        Some('0'..='9') => {
            if let Some((len, rest)) = encoded_value.split_once(':') {
                if let Ok(len) = len.parse::<usize>() {
                    return (rest[..len].to_string().into(), &rest[len..]);
                }
            }
        }
        _ => {}
    }
    panic!("Unhandled encoded value: {}", encoded_value);
}

// Usage: your_bittorrent.sh decode "<encoded_value>"
fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    match args.commands {
        Commands::Decode { value } => {
            let v = decode_bencoded(&value).0;
            println!("{v}");
        }
        Commands::Info { torrent } => {
            let f = std::fs::read(torrent).context("read torrent file")?;
            let t: Torrent = serde_bencode::from_bytes(&f).context("parse torrent file")?;
            eprintln!("{t:?}");
            println!("Tracker URL: {}", t.announce);
            if let Keys::SingleFile { length } = t.info.keys {
                println!("Length: {}", length);
            } else {
                todo!()
            }
        }
    }
    Ok(())

    // let args: Vec<String> = env::args().collect();
    // let command = &args[1];
    //
    // if command == "decode" {
    //     // You can use print statements as follows for debugging, they'll be visible when running tests.
    //     eprintln!("Logs from your program will appear here!");
    //
    //     // Uncomment this block to pass the first stage
    //     let encoded_value = &args[2];
    //     let decoded_value = decode_bencoded_value(encoded_value);
    //     println!("{}", decoded_value.0);
    // } else {
    //     println!("unknown command: {}", args[1])
    // }
}
