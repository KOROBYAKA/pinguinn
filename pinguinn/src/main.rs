pub mod cli;
use csv::Writer;
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
use tokio::sync::Mutex;

#[derive(Clone)]
struct Node {
    ip_addr: String,
    port: u16,
    delay: u128,
}

impl Node {
    fn get_ipv4(&self) -> Ipv4Addr {
        self.ip_addr.parse().unwrap()
    }

    fn get_ip(&self) -> IpAddr {
        self.ip_addr.parse().unwrap()
    }

    async fn native_ping(&mut self) {
        match ping::new(self.get_ip())
            .timeout(Duration::from_secs(1))
            .ttl(128)
            .socket_type(ping::RAW) //Keep a socket RAW to make it work normally
            .send()
        {
            Ok(packet) => {
                let delay: u128 = packet.rtt.as_millis();
                //println!("RTT {:?} IP: {:?}", delay, packet.target);
                self.delay = delay;
            }
            Err(e) => {
                //println!("Ping failed: {}", e);
                self.delay = 0;
            }
        }
    }
}

fn parse_file(file_name: &str) -> Vec<Arc<Mutex<Node>>> {
    let file = File::open(file_name).unwrap();
    let reader = BufReader::new(file);

    let mut nodes: Vec<Arc<Mutex<Node>>> = Vec::new();
    for line_w in reader.lines().skip(2) {
        let line = line_w.unwrap();
        let args: Vec<&str> = line.split('|').map(|ch| ch.trim()).collect();
        //println!("ARGS::{:?}\n", args);
        if args.len() == 1 {
            break;
        }
        let node = Arc::new(Mutex::new(Node {
            ip_addr: args[0].into(),
            port: match args[4].parse::<u16>() {
                Ok(port) => port,
                Err(_) => args[2].parse::<u16>().unwrap(),
            },
            delay: 0,
        }));
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

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    //let semaphore = Arc::new(tokio::sync::Semaphore::new(64));
    //let mut handles = Vec::new();
    //let output_file = cli::build_cli_parameters();
    //let mut handles = Vec::new();

    let nodes: Vec<Arc<Mutex<Node>>> = parse_file("gossip.log");
    for node in &nodes {
        let node_m = node.clone();
        let _ = tokio::spawn(async move {
            let mut locked_node = node_m.lock().await;
            locked_node.native_ping().await;
        });
        //handles.push(handle);
    }

    /*for handle in handles {
        let _ = handle.await;
    }

    for node in &nodes {
        let locked_node = node.lock().await;
        println!("Node IP: {:?} RTT: {:?}", locked_node.ip_addr, locked_node.delay
        );
    }*/

    let file = File::create("ping.log").unwrap();
    let mut csv_writer = csv::Writer::from_writer(file);
    csv_writer.write_record(&["IP", "RTT"]).unwrap();
    let nodes_len: usize = nodes.len();
    for node in nodes {
        let locked_node = node.lock().await;
        let ip_addr: String = locked_node.ip_addr.clone().into();
        csv_writer.serialize((ip_addr, locked_node.delay)).unwrap();
    }
    let _ = csv_writer.flush();
    println!(
        "Ping-survey is done, total nodes were pinged:{:?}",
        nodes_len
    );
}
