use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tucdp::start(tucdp::Config::Clinet {
        incoming: "127.0.0.1:2000".parse().unwrap(),
        tunnel_tcp: "127.0.0.1:2001".parse().unwrap(),
        tunnel_udp_remote: "127.0.0.1:2001".parse().unwrap(),
        tunnel_udp_local: "127.0.0.1:2002".parse().unwrap(),
        num_tcp_chans: 1,
        num_udp_chans: 0,
    })
    .await
}
