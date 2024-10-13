use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tokio::spawn(async {
        tucdp::start(tucdp::Config::Clinet {
            incoming: "0.0.0.0:3234".parse().unwrap(),
            tunnel_tcp: "127.0.0.5:2101".parse().unwrap(),
            tunnel_udp_remote: "127.0.0.5:2102".parse().unwrap(),
            tunnel_udp_local: "0.0.0.0:0".parse().unwrap(),
            num_tcp_chans: 5,
            num_udp_chans: 5,
        }).await.unwrap();
    });

    tokio::spawn(tucdp::start(tucdp::Config::Server {
        incoming_tcp: "127.0.0.5:2101".parse().unwrap(),
        incoming_udp: "127.0.0.5:2102".parse().unwrap(),
        outgoing_remote: "127.0.0.1:5959".parse().unwrap(),
        outgoing_local: "0.0.0.0:0".parse().unwrap(),
    })).await.unwrap().unwrap();



    Ok(())
}
