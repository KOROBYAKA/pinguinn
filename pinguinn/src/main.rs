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
    time,
};
use tokio::{
    sync::{mpsc, Mutex},
    task::{JoinHandle, JoinSet},
};
#[derive(Clone)]
struct Node {
    ip_addr: String,
    port: u16,
    delay: u128,
    id: String,
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
            id: args[1].into(),
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
    let (tx, mut rx) = mpsc::channel::<Node>(100);
    //let semaphore = Arc::new(tokio::sync::Semaphore::new(64));
    //let output_file = cli::build_cli_parameters();
    let nodes: Vec<Arc<Mutex<Node>>> = parse_file("gossip.log");
    let mut bad_nodes: Vec<Node> = Vec::new();
    let mut handles = JoinSet::new();
    let stats_handle = tokio::spawn(async move {
        let mut sum: u128 = 0;
        let mut failed: u128 = 0;
        let mut success: u128 = 0;
        while let Some(node) = rx.recv().await {
            if node.delay == 0 {
                failed += 1;
            } else {
                success += 1;
                sum += node.delay as u128;
            }
        }

        (sum, failed, success)
    });

    for node in &nodes {
        let node_m = node.clone();
        let tx_clone = tx.clone();
        handles.spawn(tokio::spawn(async move {
            let mut locked_node = node_m.lock().await;
            locked_node.native_ping().await;
            let _ = tx_clone.send(locked_node.clone()).await;
        }));
    }

    while handles.join_next().await.is_some() {}
    drop(tx);
    let (sum, failed, success) = stats_handle.await.unwrap();
    let avg = sum / success;
    println!("Failed:{} Success:{} AVG:{}ms", failed, success, avg);
    let cut_off: u128 = ((avg as f64) * 1.3).round() as u128;
    println!("bad delay is {} with avg delay {}", cut_off, avg);
    for node in &nodes {
        let locked_node = node.lock().await;
        if locked_node.delay > cut_off {
            bad_nodes.push(locked_node.clone());
        }
    }

    let file = File::create("ping.csv").unwrap();
    let bad_nodes_file = File::create("bad_nodes.csv").unwrap();
    let mut csv_writer = csv::Writer::from_writer(file);
    csv_writer.write_record(&["ID", "IP", "RTT"]).unwrap();
    let nodes_len: usize = nodes.len();
    for node in nodes {
        let locked_node = node.lock().await;
        let ip_addr: String = locked_node.ip_addr.clone().into();
        csv_writer
            .serialize((locked_node.id.clone(), ip_addr, locked_node.delay))
            .unwrap();
    }
    let _ = csv_writer.flush();
    let mut csv_writer = csv::Writer::from_writer(bad_nodes_file);
    let bad_nodes_len: usize = bad_nodes.len();
    for node in bad_nodes {
        let ip_addr: String = node.ip_addr.clone().into();
        csv_writer
            .serialize((node.id.clone(), ip_addr, node.delay))
            .unwrap();
    }
    let _ = csv_writer.flush();

    println!(
        "Ping-survey is done, total nodes were pinged:{}\nBad nodes(30% RTT higher) are:{}",
        nodes_len, bad_nodes_len
    );
}
