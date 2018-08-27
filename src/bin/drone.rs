extern crate bincode;
extern crate bytes;
#[macro_use]
extern crate clap;
extern crate serde_json;
extern crate solana;
extern crate tokio;
extern crate tokio_codec;

use bincode::{deserialize, serialize};
use bytes::Bytes;
use clap::{App, Arg};
use solana::crdt::NodeInfo;
use solana::drone::{Drone, DroneRequest, DRONE_PORT};
use solana::fullnode::Config;
use solana::logger;
use solana::metrics::set_panic_hook;
use solana::nat::get_public_ip_addr;
use solana::signature::read_keypair;
use solana::thin_client::poll_gossip_for_leader;
use std::error;
use std::fs::File;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::process::exit;
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::net::TcpListener;
use tokio::prelude::*;
use tokio_codec::{BytesCodec, Decoder};

fn main() -> Result<(), Box<error::Error>> {
    logger::setup();
    set_panic_hook("drone");
    let matches = App::new("drone")
        .version(crate_version!())
        .arg(
            Arg::with_name("leader")
                .short("l")
                .long("leader")
                .value_name("PATH")
                .takes_value(true)
                .help("/path/to/leader.json"),
        )
        .arg(
            Arg::with_name("keypair")
                .short("k")
                .long("keypair")
                .value_name("PATH")
                .takes_value(true)
                .required(true)
                .help("/path/to/mint.json"),
        )
        .arg(
            Arg::with_name("slice")
                .long("slice")
                .value_name("SECONDS")
                .takes_value(true)
                .help("Time slice over which to limit requests to drone"),
        )
        .arg(
            Arg::with_name("cap")
                .long("cap")
                .value_name("NUMBER")
                .takes_value(true)
                .help("Request limit for time slice"),
        )
        .arg(
            Arg::with_name("timeout")
                .long("timeout")
                .value_name("SECONDS")
                .takes_value(true)
                .help("Max SECONDS to wait to get necessary gossip from the network"),
        )
        .arg(
            Arg::with_name("addr")
                .short("a")
                .long("addr")
                .value_name("IPADDR")
                .takes_value(true)
                .help("address to advertise to the network"),
        )
        .get_matches();

    let addr = if let Some(s) = matches.value_of("addr") {
        s.to_string().parse().unwrap_or_else(|e| {
            eprintln!("failed to parse {} as IP address error: {:?}", s, e);
            exit(1);
        })
    } else {
        get_public_ip_addr().unwrap_or_else(|e| {
            eprintln!("failed to get public IP, try --addr? error: {:?}", e);
            exit(1);
        })
    };

    let leader: NodeInfo;
    if let Some(l) = matches.value_of("leader") {
        leader = read_leader(l).node_info;
    } else {
        let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 8000);
        leader = NodeInfo::new_leader(&server_addr);
    };

    let mint_keypair =
        read_keypair(matches.value_of("keypair").expect("keypair")).expect("client keypair");

    let time_slice: Option<u64>;
    if let Some(secs) = matches.value_of("slice") {
        time_slice = Some(secs.to_string().parse().expect("integer"));
    } else {
        time_slice = None;
    }
    let request_cap: Option<u64>;
    if let Some(c) = matches.value_of("cap") {
        request_cap = Some(c.to_string().parse().expect("integer"));
    } else {
        request_cap = None;
    }
    let timeout: Option<u64>;
    if let Some(secs) = matches.value_of("timeout") {
        timeout = Some(secs.to_string().parse().expect("integer"));
    } else {
        timeout = None;
    }

    let leader = poll_gossip_for_leader(leader.contact_info.ncp, timeout, addr)?;

    let drone_addr: SocketAddr = format!("0.0.0.0:{}", DRONE_PORT).parse().unwrap();

    let drone = Arc::new(Mutex::new(Drone::new(
        mint_keypair,
        drone_addr,
        leader.contact_info.tpu,
        leader.contact_info.rpu,
        time_slice,
        request_cap,
    )));

    let drone1 = drone.clone();
    thread::spawn(move || loop {
        let time = drone1.lock().unwrap().time_slice;
        thread::sleep(time);
        drone1.lock().unwrap().clear_request_count();
    });

    let socket = TcpListener::bind(&drone_addr).unwrap();
    println!("Drone started. Listening on: {}", drone_addr);
    let done = socket
        .incoming()
        .map_err(|e| println!("failed to accept socket; error = {:?}", e))
        .for_each(move |socket| {
            let drone2 = drone.clone();
            // let client_ip = socket.peer_addr().expect("drone peer_addr").ip();
            let framed = BytesCodec::new().framed(socket);
            let (writer, reader) = framed.split();

            let processor = reader.and_then(move |bytes| {
                let req: DroneRequest = deserialize(&bytes).or_else(|err| {
                    Err(io::Error::new(
                        io::ErrorKind::Other,
                        format!("deserialize packet in drone: {:?}", err),
                    ))
                })?;

                println!("Airdrop requested...");
                // let res = drone2.lock().unwrap().check_rate_limit(client_ip);
                let res1 = drone2.lock().unwrap().send_airdrop(req);
                match res1 {
                    Ok(_) => println!("Airdrop sent!"),
                    Err(_) => println!("Request limit reached for this time slice"),
                }
                let response = res1?;
                println!("Airdrop tx signature: {:?}", response);
                let response_vec = serialize(&response).or_else(|err| {
                    Err(io::Error::new(
                        io::ErrorKind::Other,
                        format!("serialize signature in drone: {:?}", err),
                    ))
                })?;
                let response_bytes = Bytes::from(response_vec.clone());
                Ok(response_bytes)
            });
            let server = writer
                .send_all(processor.or_else(|err| {
                    Err(io::Error::new(
                        io::ErrorKind::Other,
                        format!("Drone response: {:?}", err),
                    ))
                }))
                .then(|_| Ok(()));
            tokio::spawn(server)
        });
    tokio::run(done);
    Ok(())
}

fn read_leader(path: &str) -> Config {
    let file = File::open(path).unwrap_or_else(|_| panic!("file not found: {}", path));
    serde_json::from_reader(file).unwrap_or_else(|_| panic!("failed to parse {}", path))
}
