use csv::Writer;
use ping;
use pinguinn::{
    cli::{parse_args, CliArgs},
    quic_networking::{create_client_config, create_client_endpoint, QuicClientCertificate},
};
use quinn::Endpoint;
use solana_sdk::signature::Keypair;
use std::net::SocketAddr;
use std::thread::sleep;
use std::time::{Duration, Instant};
use std::{
    fs::File,
    io::{BufRead, BufReader},
    net::IpAddr,
    sync::Arc,
};
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tokio::{
    sync::{mpsc, Mutex},
    task::JoinSet,
};

#[derive(Clone)]
struct Node {
    ip_addr: String,
    tpu_port: u16,
    delay: u128,
    quic_delay: u128,
    id: String,
    retries: u16,
    connection_timeout: u64,
}

impl Node {
    /*    fn get_ipv4(&self) -> Ipv4Addr {
            self.ip_addr.parse().unwrap()
        }
    */
    fn get_ip(&self) -> IpAddr {
        self.ip_addr.parse().unwrap()
    }

    async fn quic_ping(&mut self, endpoint: Arc<Endpoint>) {
        let remote: SocketAddr = format!("{}:{}", self.ip_addr, self.tpu_port)
            .parse()
            .unwrap();
        for _ in 0..self.retries {
            let start = Instant::now();
            let endpoint_connection = endpoint.connect(remote, "solana-tpu").unwrap();
            //println!("Ping IP{:?}:{:?}", self.ip_addr, self.port);
            let result = timeout(
                Duration::from_millis(self.connection_timeout),
                endpoint_connection,
            )
            .await;
            let delay = start.elapsed().as_millis();
            match result {
                Ok(Ok(_)) => {
                    self.quic_delay = delay as u128;
                    break;
                }
                _ => (),
            }
            sleep(Duration::from_millis(1000));
        }
        println!(
            "LOOP DEAD IP{:?} QUIC_RTT:{:?}",
            self.ip_addr, self.quic_delay
        );
    }

    async fn native_ping(&mut self) {
        match ping::new(self.get_ip())
            .timeout(Duration::from_millis(self.connection_timeout))
            .ttl(128)
            .socket_type(ping::RAW) //Keep a socket RAW to make it work normally
            .send()
        {
            Ok(packet) => {
                let delay: u128 = packet.rtt.as_millis();
                println!("RTT {:?} IP: {:?}", delay, packet.target);
                self.delay = delay;
            }
            Err(_) => {
                //println!("Ping failed: {}", e);
                self.delay = 0;
            }
        }
    }
}

fn parse_file(cli_args: &CliArgs) -> Vec<Arc<Mutex<Node>>> {
    let file = File::open(cli_args.input_file.clone()).unwrap();
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
            tpu_port: match args[4].parse::<u16>() {
                Ok(port) => port,
                Err(_) => continue,
            },
            delay: 0,
            quic_delay: 0,
            id: args[1].into(),
            retries: cli_args.retries,
            connection_timeout: cli_args.connection_timeout,
        }));
        nodes.push(node);
    }
    nodes
}

/*async fn icmp_ping(node: &Node) {
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
}*/

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("failed to install rustls crypto provider");
    //Creating a single QUIC endpoint for all nodes

    let keypair = Keypair::new();
    let client_cert: Arc<QuicClientCertificate> = Arc::new(QuicClientCertificate::new(&keypair));
    let bind_addr: SocketAddr = "0.0.0.0:0".parse().unwrap();
    let client_config: quinn::ClientConfig = create_client_config(client_cert, false);
    let quic_endpoint: Arc<Endpoint> =
        Arc::new(create_client_endpoint(bind_addr, client_config).unwrap());

    //Parsing CLI args
    let args = parse_args();

    //
    let (quic_tx, mut quic_rx) = mpsc::channel::<u128>(args.concurrent_connections);
    let (icmp_tx, mut icmp_rx) = mpsc::channel::<u128>(args.concurrent_connections);

    let icmp_semaphore = Arc::new(Semaphore::new(args.concurrent_connections));
    let quic_semaphore = Arc::new(Semaphore::new(args.concurrent_connections));
    let nodes: Vec<Arc<Mutex<Node>>> = parse_file(&args);
    println!("Gossip TPU nodes:{}", nodes.len());
    let mut bad_nodes: Vec<Node> = Vec::new();
    //let mut handles = JoinSet::new();
    let icmp_stats_handle = tokio::spawn(async move {
        let mut sum: usize = 0;
        let mut failed: usize = 0;
        let mut success: usize = 0;
        while let Some(delay) = icmp_rx.recv().await {
            if delay == 0 {
                failed += 1;
            } else {
                success += 1;
                sum += delay as usize;
            }
        }

        (sum, failed, success)
    });
    let quic_stats_handle = tokio::spawn(async move {
        let mut sum: usize = 0;
        let mut failed: usize = 0;
        let mut success: usize = 0;
        while let Some(delay) = quic_rx.recv().await {
            if delay == 0 {
                failed += 1;
            } else {
                success += 1;
                sum += delay as usize;
            }
        }

        (sum, failed, success)
    });
    let mut quic_handles = JoinSet::new();
    let mut icmp_handles = JoinSet::new();

    match args.mode {
        1 =>
        //ICMP RTT
        {
            /*            drop(quic_endpoint);
                        drop(quic_handles);
                        drop(quic_rx);
                        drop(quic_stats_handle);
                        drop(quic_tx);
            */
            for node in &nodes {
                let semaphore_clone = icmp_semaphore.clone();
                let node_m = node.clone();
                let tx_clone = icmp_tx.clone();
                icmp_handles.spawn(tokio::spawn(async move {
                    let _permit = semaphore_clone.acquire().await.unwrap();
                    let mut locked_node = node_m.lock().await;
                    locked_node.native_ping().await;
                    let _ = tx_clone.send(locked_node.quic_delay).await;
                    drop(_permit);
                }));
            }
            while icmp_handles.join_next().await.is_some() {}
            drop(icmp_tx);
        }
        2 =>
        //QUIC RTT
        {
            /*
            drop(icmp_handles);
            drop(icmp_rx);
            drop(icmp_stats_handle);
            drop(icmp_tx);
            */
            for node in &nodes {
                let semaphore_clone = quic_semaphore.clone();
                let node_m = node.clone();
                let tx_clone = quic_tx.clone();
                let endpoint_clone = quic_endpoint.clone();
                quic_handles.spawn(tokio::spawn(async move {
                    let _permit = semaphore_clone.acquire().await.unwrap();
                    let mut locked_node = node_m.lock().await;
                    locked_node.quic_ping(endpoint_clone).await;
                    let _ = tx_clone.send(locked_node.quic_delay).await;
                    drop(_permit);
                }));
            }
            while quic_handles.join_next().await.is_some() {}
            drop(quic_tx);
        }
        3 =>
        //Both measurements
        {
            for node in &nodes {
                let semaphore_clone = quic_semaphore.clone();
                let node_m = node.clone();
                let tx_clone = quic_tx.clone();
                let endpoint_clone = quic_endpoint.clone();
                quic_handles.spawn(tokio::spawn(async move {
                    let _permit = semaphore_clone.acquire().await.unwrap();
                    let mut locked_node = node_m.lock().await;
                    locked_node.quic_ping(endpoint_clone).await;
                    let _ = tx_clone.send(locked_node.quic_delay).await;
                    drop(_permit);
                }));
            }
            while quic_handles.join_next().await.is_some() {}
            drop(quic_tx);
            for node in &nodes {
                let semaphore_clone = icmp_semaphore.clone();
                let node_m = node.clone();
                let tx_clone = icmp_tx.clone();
                let endpoint_clone = quic_endpoint.clone();
                icmp_handles.spawn(tokio::spawn(async move {
                    let _permit = semaphore_clone.acquire().await.unwrap();
                    let mut locked_node = node_m.lock().await;
                    locked_node.quic_ping(endpoint_clone).await;
                    let _ = tx_clone.send(locked_node.quic_delay).await;
                    drop(_permit);
                }));
            }
            while icmp_handles.join_next().await.is_some() {}
            drop(icmp_tx);
        }
        _ => {
            println!("Mode value is not valid!");
            std::process::exit(0);
        }
    }

    //Printing stats
    match args.mode {
        1 => {
            let (sum, failed, success) = icmp_stats_handle.await.unwrap();
            let avg = sum / success;
            println!(
                "ICMP stats\nFailed:{} Success:{} AVG:{}ms",
                failed, success, avg
            );
            let cut_off: u128 = ((avg as f64) * 1.3).round() as u128;
            println!("ICMP: Bad delay is {} with avg delay {}", cut_off, avg);
            for node in &nodes {
                let locked_node = node.lock().await;
                if locked_node.delay > cut_off {
                    bad_nodes.push(locked_node.clone());
                }
            }
        }
        2 => {
            let (sum, failed, success) = quic_stats_handle.await.unwrap();
            let avg = sum / success;
            println!(
                "QUIC stats\nFailed:{} Success:{} AVG:{}ms",
                failed, success, avg
            );
            let cut_off: u128 = ((avg as f64) * 1.3).round() as u128;
            println!("Bad delay is {} with avg delay {}", cut_off, avg);
            for node in &nodes {
                let locked_node = node.lock().await;
                if locked_node.quic_delay > cut_off {
                    bad_nodes.push(locked_node.clone());
                }
            }
        }
        3 => {
            let (sum, failed, success) = icmp_stats_handle.await.unwrap();
            let avg = sum / success;
            println!(
                "QUIC stats\nFailed:{} Success:{} AVG:{}ms",
                failed, success, avg
            );
            let cut_off: u128 = ((avg as f64) * 1.3).round() as u128;
            println!("Bad delay is {} with avg delay {}", cut_off, avg);
            for node in &nodes {
                let locked_node = node.lock().await;
                if locked_node.quic_delay > cut_off {
                    bad_nodes.push(locked_node.clone());
                }
            }
            let (sum, failed, success) = quic_stats_handle.await.unwrap();
            let avg = sum / success;
            println!(
                "ICMP stats\nFailed:{} Success:{} AVG:{}ms",
                failed, success, avg
            );
            let cut_off: u128 = ((avg as f64) * 1.3).round() as u128;
            println!("ICMP: Bad delay is {} with avg delay {}", cut_off, avg);
            for node in &nodes {
                let locked_node = node.lock().await;
                if locked_node.delay > cut_off {
                    bad_nodes.push(locked_node.clone());
                }
            }
        }
        _ => (),
    }

    let file = File::create(args.output_file).unwrap();
    let bad_nodes_file = File::create("bad_nodes.csv").unwrap();
    let mut csv_writer = csv::Writer::from_writer(file);
    csv_writer
        .write_record(&["ID", "IP", "QUIC_HANDSHASKE_RTT", "ICMP_RTT"])
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
            .serialize((node.id.clone(), ip_addr, node.quic_delay, node.delay))
            .unwrap();
    }
    let _ = csv_writer.flush();

    println!(
        "Ping-survey is done, total nodes were pinged:{}\nBad nodes(30% RTT higher) are:{}",
        nodes_len, bad_nodes_len
    );
}
