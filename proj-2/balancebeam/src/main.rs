mod request;
mod response;
use clap::Parser;
use core::panic;
use rand::{Rng, SeedableRng};
use std::{collections::HashMap, sync::Arc, thread};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

/// Contains information parsed from the command-line invocation of balancebeam. The Clap macros
/// provide a fancy way to automatically construct a command-line argument parser.
#[derive(Parser, Debug)]
#[command(about = "Fun with load balancing")]
struct CmdOptions {
    // about = "IP/port to bind to",
    #[arg(short, long, default_value = "0.0.0.0:1100")]
    bind: String,
    // about = "Upstream host to forward requests to"
    #[arg(short, long)]
    upstream: Vec<String>,
    //  about = "Perform active health checks on this interval (in seconds)",
    #[arg(long, default_value = "2000")]
    active_health_check_interval: usize,
    // about = "Path to send request to for active health checks",
    #[arg(long, default_value = "/")]
    active_health_check_path: String,
    // about = "Maximum number of requests to accept per IP per minute (0 = unlimited)",
    #[arg(long, default_value = "100")]
    max_requests_per_minute: usize,
}

/// Contains information about the state of balancebeam (e.g. what servers we are currently proxying
/// to, what servers have failed, rate limiting counts, etc.)
///
/// You should add fields to this struct in later milestones.
struct ProxyState {
    /// How frequently we check whether upstream servers are alive (Milestone 4)
    #[allow(dead_code)]
    active_health_check_interval: usize,
    /// Where we should send requests when doing active health checks (Milestone 4)
    #[allow(dead_code)]
    active_health_check_path: String,
    /// Maximum number of requests an individual IP can make in a minute (Milestone 5)
    #[allow(dead_code)]
    max_requests_per_minute: usize,
    /// Addresses of servers that we are proxying to
    upstream_addresses: Arc<Mutex<Vec<String>>>,
    /// Addresses of servers that currently failed
    failed_upstream_addresses: Arc<Mutex<Vec<String>>>,
    request_counter: Arc<Mutex<HashMap<String, usize>>>,
}
#[tokio::main]
async fn main() {
    // Initialize the logging library. You can print log messages using the `log` macros:
    // https://docs.rs/log/0.4.8/log/ You are welcome to continue using print! statements; this
    // just looks a little prettier.
    if let Err(_) = std::env::var("RUST_LOG") {
        std::env::set_var("RUST_LOG", "debug");
    }
    pretty_env_logger::init();

    // Parse the command line arguments passed to this program
    let options = CmdOptions::parse();
    if options.upstream.len() < 1 {
        log::error!("At least one upstream server must be specified using the --upstream option.");
        std::process::exit(1);
    }

    // Start listening for connections
    let listener = match TcpListener::bind(&options.bind).await {
        Ok(listener) => listener,
        Err(err) => {
            log::error!("Could not bind to {}: {}", options.bind, err);
            std::process::exit(1);
        }
    };
    log::info!("Listening for requests on {}", options.bind);

    // Handle incoming connections
    let state = Arc::new(ProxyState {
        upstream_addresses: Arc::new(Mutex::new(options.upstream)),
        failed_upstream_addresses: Arc::new(Mutex::new(vec![])),
        active_health_check_interval: options.active_health_check_interval,
        active_health_check_path: options.active_health_check_path,
        max_requests_per_minute: options.max_requests_per_minute,
        request_counter: Arc::new(Mutex::new(HashMap::new())),
    });
    spawn_active_check(state.clone()).await;
    spawn_counter_clearer(state.clone()).await;

    while let Ok((stream, _)) = listener.accept().await {
        let local_state = state.clone();
        // 启动一个新的异步任务来处理连接
        tokio::task::spawn(async move {
            handle_connection(stream, local_state).await;
        });
        // Handle the connection!
    }
}

async fn spawn_counter_clearer(state: Arc<ProxyState>) {
    let arc_counter = state.request_counter.clone();
    tokio::task::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            arc_counter.lock().await.clear();
        }
    });
}

async fn spawn_active_check(state: Arc<ProxyState>) {
    let local_arc_alive = state.upstream_addresses.clone();
    let local_arc_failed = state.failed_upstream_addresses.clone();
    let health_path = state.active_health_check_path.clone();
    let interval = state.active_health_check_interval;
    tokio::task::spawn(async move {
        loop {
            thread::sleep(std::time::Duration::from_millis(interval as u64));
            let mut ips: Vec<String> = local_arc_alive.lock().await.clone();
            ips.extend(local_arc_failed.lock().await.clone());
            let mut new_alive = vec![];
            let mut new_failed = vec![];
            for ele in ips {
                if is_active(&ele, &health_path).await {
                    new_alive.push(ele);
                } else {
                    new_failed.push(ele);
                }
            }
            {
                let mut alive = local_arc_alive.lock().await;
                alive.clear();
                alive.extend(new_alive);
            }
            {
                let mut failed = local_arc_failed.lock().await;
                failed.clear();
                failed.extend(new_failed);
            }
        }
    });
}

async fn is_active(ip: &String, path: &String) -> bool {
    let request = http::Request::builder()
        .method(http::Method::GET)
        .uri(path)
        .header("Host", ip)
        .body(Vec::new())
        .unwrap();
    match TcpStream::connect(&ip).await {
        Ok(mut connection) => match request::write_to_stream(&request, &mut connection).await {
            Ok(_) => match response::read_from_stream(&mut connection, request.method()).await {
                Ok(response) => response.status().as_u16() == 200,
                Err(_) => false,
            },
            Err(_) => false,
        },
        Err(_) => false,
    }
}

async fn connect_to_upstream_once(state: Arc<ProxyState>) -> Result<TcpStream, std::io::Error> {
    let upstream_ip = {
        let mut rng = rand::rngs::StdRng::from_entropy();
        let upstream_ref = state.upstream_addresses.lock().await;
        if upstream_ref.is_empty() {
            panic!("no available sub ");
        }
        let upstream_idx = rng.gen_range(0, upstream_ref.len());
        upstream_ref[upstream_idx].clone()
    };
    // 此前的锁只是为了从state中读一个ip出来, 在有io时不要给upstream数组上锁
    match TcpStream::connect(&upstream_ip).await {
        Ok(res) => Ok(res),
        Err(err) => {
            log::error!("Failed to connect to upstream {}: {}", &upstream_ip, err);
            // passive fail over
            // upstream_ip lifecycle is over,so we can use state as mut ref;
            let mut upstreams = state.upstream_addresses.lock().await;
            if let Some(idx) = upstreams.iter().position(|x| *x == upstream_ip) {
                upstreams.swap_remove(idx);
            }
            state
                .failed_upstream_addresses
                .lock()
                .await
                .push(upstream_ip);
            Err(err)
        }
    }
}

async fn connect_to_upstream(state: Arc<ProxyState>) -> Result<TcpStream, std::io::Error> {
    let mut res = connect_to_upstream_once(state.clone()).await;
    while res.is_err() {
        res = connect_to_upstream_once(state.clone()).await;
    }
    res
}

async fn send_response(client_conn: &mut TcpStream, response: &http::Response<Vec<u8>>) {
    let client_ip = client_conn.peer_addr().unwrap().ip().to_string();
    log::info!(
        "{} <- {}",
        client_ip,
        response::format_response_line(&response)
    );
    if let Err(error) = response::write_to_stream(&response, client_conn).await {
        log::warn!("Failed to send response to client: {}", error);
        return;
    }
}

async fn handle_connection(mut client_conn: TcpStream, state: Arc<ProxyState>) {
    let client_ip = client_conn.peer_addr().unwrap().ip().to_string();
    log::info!("Connection received from {}", client_ip);

    // Open a connection to a random destination server
    let mut upstream_conn = match connect_to_upstream(state.clone()).await {
        Ok(stream) => stream,
        Err(_error) => {
            let response = response::make_http_error(http::StatusCode::BAD_GATEWAY);
            send_response(&mut client_conn, &response).await;
            return;
        }
    };
    let upstream_ip = client_conn.peer_addr().unwrap().ip().to_string();

    // The client may now send us one or more requests. Keep trying to read requests until the
    // client hangs up or we get an error.
    loop {
        // Read a request from the client
        let mut request = match request::read_from_stream(&mut client_conn).await {
            Ok(request) => {
                let mut counter = state.request_counter.lock().await;
                if !counter.contains_key(&client_ip) {
                    counter.insert(client_ip.clone(), 0);
                }
                let request_times = *counter.get(&client_ip).unwrap();
                if request_times >= state.max_requests_per_minute {
                    let response = response::make_http_error(http::StatusCode::TOO_MANY_REQUESTS);
                    send_response(&mut client_conn, &response).await;
                    continue;
                } else {
                    counter.insert(client_ip.clone(), request_times + 1);
                }
                request
            }
            // Handle case where client closed connection and is no longer sending requests
            Err(request::Error::IncompleteRequest(0)) => {
                log::debug!("Client finished sending requests. Shutting down connection");
                return;
            }
            // Handle I/O error in reading from the client
            Err(request::Error::ConnectionError(io_err)) => {
                log::info!("Error reading request from client stream: {}", io_err);
                return;
            }
            Err(error) => {
                log::debug!("Error parsing request: {:?}", error);
                let response = response::make_http_error(match error {
                    request::Error::IncompleteRequest(_)
                    | request::Error::MalformedRequest(_)
                    | request::Error::InvalidContentLength
                    | request::Error::ContentLengthMismatch => http::StatusCode::BAD_REQUEST,
                    request::Error::RequestBodyTooLarge => http::StatusCode::PAYLOAD_TOO_LARGE,
                    request::Error::ConnectionError(_) => http::StatusCode::SERVICE_UNAVAILABLE,
                });
                send_response(&mut client_conn, &response).await;
                continue;
            }
        };
        log::info!(
            "{} -> {}: {}",
            client_ip,
            upstream_ip,
            request::format_request_line(&request)
        );

        // Add X-Forwarded-For header so that the upstream server knows the client's IP address.
        // (We're the ones connecting directly to the upstream server, so without this header, the
        // upstream server will only know our IP, not the client's.)
        request::extend_header_value(&mut request, "x-forwarded-for", &client_ip);

        // Forward the request to the server
        if let Err(error) = request::write_to_stream(&request, &mut upstream_conn).await {
            log::error!(
                "Failed to send request to upstream {}: {}",
                upstream_ip,
                error
            );
            let response = response::make_http_error(http::StatusCode::BAD_GATEWAY);
            send_response(&mut client_conn, &response).await;
            return;
        }
        log::debug!("Forwarded request to server");

        // Read the server's response
        let response = match response::read_from_stream(&mut upstream_conn, request.method()).await
        {
            Ok(response) => response,
            Err(error) => {
                log::error!("Error reading response from server: {:?}", error);
                let response = response::make_http_error(http::StatusCode::BAD_GATEWAY);
                send_response(&mut client_conn, &response).await;
                return;
            }
        };
        // Forward the response to the client
        send_response(&mut client_conn, &response).await;
        log::debug!("Forwarded response to client");
    }
}
