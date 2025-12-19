pub mod cli;
pub mod error;
pub mod quic_networking;
pub mod tls_certificates;
use csv::Writer;
use icmp;
use ping;
use quic_networking::{create_client_config, create_client_endpoint, QuicClientCertificate};
use quinn::Endpoint;
use solana_sdk::signature::Keypair;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use std::{
    fs::File,
    io::{BufRead, BufReader},
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
    time,
};
use tokio::time::timeout;
use tokio::{
    sync::{mpsc, Mutex},
    task::{JoinHandle, JoinSet},
};

#[derive(Clone)]
struct Node {
    ip_addr: String,
    port: u16,
    delay: u128,
    quic_delay: u128,
    id: String,
}

impl Node {
    fn get_ipv4(&self) -> Ipv4Addr {
        self.ip_addr.parse().unwrap()
    }

    fn get_ip(&self) -> IpAddr {
        self.ip_addr.parse().unwrap()
    }

    async fn quic_ping(&mut self, endpoint: Arc<Endpoint>) {
        let remote: SocketAddr = format!("{}:{}", self.ip_addr, self.port).parse().unwrap();
        let start = Instant::now();
        let endpoint_connection = endpoint.connect(remote, "solana-tpu").unwrap();
        println!("Ping IP{:?}:{:?}", self.ip_addr, self.port);
        let result = timeout(Duration::from_millis(5000), endpoint_connection).await;
        match result {
            Ok(Ok(_)) => self.quic_delay = start.elapsed().as_millis() as u128,
            _ => (),
        }
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
            quic_delay: 0,
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
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("failed to install rustls crypto provider");
    //Creating a single QUIC endpoint for all nodes
    let keypair = Keypair::new();
    let client_cert: Arc<QuicClientCertificate> = Arc::new(QuicClientCertificate::new(&keypair));
    let bind_addr = "0.0.0.0:0".parse().unwrap();
    let client_config: quinn::ClientConfig = create_client_config(client_cert, false);
    let quic_endpoint: Arc<Endpoint> =
        Arc::new(create_client_endpoint(bind_addr, client_config).unwrap());

    //
    let (tx, mut rx) = mpsc::channel::<Node>(100);
    let nodes: Vec<Arc<Mutex<Node>>> = parse_file("gossip.log");
    let mut bad_nodes: Vec<Node> = Vec::new();
    let mut handles = JoinSet::new();
    let stats_handle = tokio::spawn(async move {
        let mut sum: u128 = 0;
        let mut failed: u128 = 0;
        let mut success: u128 = 1;
        while let Some(node) = rx.recv().await {
            if node.quic_delay == 0 {
                failed += 1;
            } else {
                success += 1;
                sum += node.quic_delay as u128;
            }
        }

        (sum, failed, success)
    });

    for node in &nodes {
        let node_m = node.clone();
        let tx_clone = tx.clone();
        let endpoint_clone = quic_endpoint.clone();
        handles.spawn(tokio::spawn(async move {
            let mut locked_node = node_m.lock().await;
            locked_node.quic_ping(endpoint_clone).await;
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
    csv_writer
        .write_record(&["ID", "IP", "QUIC_HANDSHASKE", "ICMP_RTT"])
        .unwrap();
    let nodes_len: usize = nodes.len();
    for node in nodes {
        let locked_node = node.lock().await;
        let ip_addr: String = locked_node.ip_addr.clone().into();
        csv_writer
            .serialize((
                locked_node.id.clone(),
                ip_addr,
                locked_node.quic_delay,
                locked_node.delay,
            ))
            .unwrap();
    }
    let _ = csv_writer.flush();
    let mut csv_writer = Writer::from_writer(bad_nodes_file);
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
