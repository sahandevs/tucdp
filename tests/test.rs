use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

struct MockClient {
    messages: Arc<Mutex<Vec<Vec<u8>>>>,
    sock: Arc<tokio::net::UdpSocket>,
}

impl MockClient {
    async fn connect_to(addr_local: SocketAddr, addr_remote: SocketAddr) -> Self {
        let sock = Arc::new(tokio::net::UdpSocket::bind(addr_local).await.unwrap());
        sock.connect(addr_remote).await.unwrap();

        let sock_ref = sock.clone();
        let messages = Arc::new(Mutex::new(Vec::new()));
        let messages_ref = messages.clone();

        tokio::spawn(async move {
            let mut buf = [0u8; 65536];
            loop {
                let n = sock_ref.recv(&mut buf).await.unwrap();
                let data = &buf[..n];
                messages_ref.lock().unwrap().push(data.to_vec());
            }
        });

        Self { messages, sock }
    }

    async fn send(&self, data: &[u8]) {
        self.sock.send(data).await.unwrap();
    }
}

struct MockServer {
    messages: Arc<Mutex<Vec<Vec<u8>>>>,
    #[allow(dead_code)]
    sock: Arc<tokio::net::UdpSocket>,
}

impl MockServer {
    async fn start(addr: SocketAddr) -> Self {
        let sock = Arc::new(tokio::net::UdpSocket::bind(addr).await.unwrap());

        let sock_ref = sock.clone();
        let messages = Arc::new(Mutex::new(Vec::new()));
        let messages_ref = messages.clone();

        tokio::spawn(async move {
            let mut buf = [0u8; 65536];
            loop {
                let (n, sender) = sock_ref.recv_from(&mut buf).await.unwrap();
                let data = &buf[..n];
                messages_ref.lock().unwrap().push(data.to_vec());

                let mut data = data.to_vec();
                data.reverse();
                sock_ref.send_to(data.as_slice(), sender).await.unwrap();
            }
        });

        Self { messages, sock }
    }
}

#[tokio::test]
async fn test_it_works() {
    let server = MockServer::start("127.0.0.1:2000".parse().unwrap()).await;

    tokio::spawn(tucdp::start(tucdp::Config::Clinet {
        incoming: "127.0.0.1:3234".parse().unwrap(),
        tunnel_tcp: "127.0.0.1:2101".parse().unwrap(),
        tunnel_udp_remote: "127.0.0.1:2101".parse().unwrap(),
        tunnel_udp_local: "127.0.0.1:2004".parse().unwrap(),
    }));

    tokio::spawn(tucdp::start(tucdp::Config::Server {
        incoming_tcp: "127.0.0.1:2101".parse().unwrap(),
        incoming_udp: "127.0.0.1:2101".parse().unwrap(),
        outgoing_remote: "127.0.0.1:2000".parse().unwrap(),
        outgoing_local: "127.0.0.1:2003".parse().unwrap(),
    }));

    let client = MockClient::connect_to(
        "127.0.0.1:2005".parse().unwrap(),
        "127.0.0.1:3234".parse().unwrap(),
    )
    .await;

    assert_eq!(client.messages.lock().unwrap().len(), 0);
    assert_eq!(client.messages.lock().unwrap().len(), 0);
    client.send("hello".as_bytes()).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(server.messages.lock().unwrap().len(), 1);
    assert_eq!(
        server.messages.lock().unwrap().get(0).unwrap(),
        "hello".as_bytes()
    );

    assert_eq!(client.messages.lock().unwrap().len(), 1);
    assert_eq!(
        client.messages.lock().unwrap().get(0).unwrap(),
        "olleh".as_bytes()
    );
}
