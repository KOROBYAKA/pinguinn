pub mod cli;
use icmp;
use ping;
use std::time::{Duration, Instant};
use std::{
    fs::File,
    io::{BufRead, BufReader},
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
    thread, time,
};
use tokio;

struct Node {
    ip_addr: String,
    port: u16,
    delay: u32,
}

impl Node {
    fn get_ipv4(&self) -> Ipv4Addr {
        self.ip_addr.parse().unwrap()
    }

    fn get_ip(&self) -> IpAddr {
        self.ip_addr.parse().unwrap()
    }
}

fn parse_file(file_name: &str) -> Vec<Node> {
    let file = File::open(file_name).unwrap();
    let reader = BufReader::new(file);

    let mut nodes = Vec::new();
    for line_w in reader.lines().skip(2) {
        let line = line_w.unwrap();
        let args: Vec<&str> = line.split('|').map(|ch| ch.trim()).collect();
        println!("ARGS::{:?}\n", args);
        if args.len() == 1 {
            break;
        }
        let node = Node {
            ip_addr: args[0].into(),
            port: match args[4].parse::<u16>() {
                Ok(port) => port,
                Err(_) => args[2].parse::<u16>().unwrap(),
            },
            delay: 0,
        };
        nodes.push(node);
    }
    nodes
}

async fn icmp_ping(node: &Node) {
    let target_ip: Ipv4Addr = node.get_ipv4();
    let mut sock = icmp::IcmpSocket::connect(IpAddr::V4(target_ip)).unwrap();
    let start = Instant::now();
    let mut rcv_buf = vec![1, 2];
    let result = sock.send(&[0, 0]);
    println!("PING RESULT::{:?}", result);
    loop {
        let n = sock.recv(&mut rcv_buf).unwrap();
        if n == 0 {
            continue;
        }
        break;
    }
    let dt = start.elapsed().as_millis();
    println!("Time:{:?}", dt);
}

async fn native_ping(node: &Node) {
    match ping::new(node.get_ip())
        .timeout(Duration::from_secs(2))
        .ttl(128)
        .socket_type(ping::RAW) //Keep a socket RAW to make it work normally
        .send()
    {
        Ok(packet) => {
            let millis: u128 = packet.rtt.as_millis();
            println!("RTT {:?} IP: {:?}", millis, packet.target);
        }
        Err(e) => eprintln!("Ping failed: {}", e),
    }
}

#[tokio::main]
async fn main() {
    //let semaphore = Arc::new(tokio::sync::Semaphore::new(31));
    //let output_file = cli::build_cli_parameters();
    let nodes = parse_file("gossip.log");
    for node in nodes {
        native_ping(&node).await;
    }
}
