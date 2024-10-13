use std::{
    net::SocketAddr,
    sync::{atomic::AtomicBool, Arc},
};

use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub async fn start(cfg: Config) -> Result<()> {
    match cfg {
        Config::Clinet {
            incoming,
            tunnel_tcp,
            tunnel_udp_local,
            tunnel_udp_remote,
        } => {
            start_client(
                incoming,
                tunnel_tcp,
                tunnel_udp_remote,
                tunnel_udp_local,
                5,
                5,
            )
            .await
        }
        Config::Server {
            incoming_tcp,
            incoming_udp,
            outgoing_remote,
            outgoing_local,
        } => todo!(),
    }
}

async fn start_client(
    incoming_addr: SocketAddr,
    tunnel_tcp_addr: SocketAddr,
    tunnel_udp_addr_remote: SocketAddr,
    tunnel_udp_addr_local: SocketAddr,
    num_tcp_tuns: usize,
    num_udp_tuns: usize,
) -> Result<()> {
    let mut incoming_sock_connected = false;
    let incoming_sock = Arc::new(tokio::net::UdpSocket::bind(incoming_addr).await.unwrap());

    let mut to_tun_chans: Vec<tokio::sync::mpsc::Sender<(u16, Vec<u8>)>> = vec![];
    let mut from_tun_chans: Vec<tokio::sync::mpsc::Receiver<(u16, Vec<u8>)>> = vec![];

    for _ in 0..num_tcp_tuns {
        let (tx_o, mut rx) = tokio::sync::mpsc::channel(100);
        to_tun_chans.push(tx_o);

        let (tx, rx_o) = tokio::sync::mpsc::channel(100);
        from_tun_chans.push(rx_o);

        tokio::spawn(async move {
            loop {
                'inner: {
                    macro_rules! or_retry {
                        ($e:expr) => {
                            match $e {
                                Ok(x) => x,
                                Err(e) => {
                                    println!("[client] tcp chan got {e:?}.");
                                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                                    break 'inner;
                                }
                            }
                        };
                    }

                    let stream = or_retry!(tokio::net::TcpStream::connect(tunnel_tcp_addr).await);
                    let (mut read, mut write) = stream.into_split();

                    loop {
                        tokio::select! {
                            x = rx.recv() => {
                                let Some((id, data)) = x else { break 'inner };
                                or_retry!(write.write_u16(id).await);
                                or_retry!(write.write_u64(data.len() as _).await);
                                or_retry!(write.write_all(data.as_slice()).await);
                            },

                            x = read.read_u16() => {
                                let id = or_retry!(x);
                                let size = or_retry!(read.read_u64().await) as usize;
                                let mut buf = vec![0u8; size];
                                or_retry!(read.read_exact(&mut buf).await);
                                let _ = tx.send((id, buf));
                            }

                        }
                    }
                }
            }
        });
    }

    for _ in 0..num_udp_tuns {
        let (tx, mut rx) = tokio::sync::mpsc::channel(100);
        to_tun_chans.push(tx);

        let (tx, rx_o) = tokio::sync::mpsc::channel(100);
        from_tun_chans.push(rx_o);

        tokio::spawn(async move {
            loop {
                'inner: {
                    macro_rules! or_retry {
                        ($e:expr) => {
                            match $e {
                                Ok(x) => x,
                                Err(e) => {
                                    println!("[client] udp chan got {e:?}.");
                                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                                    break 'inner;
                                }
                            }
                        };
                    }

                    let sock = or_retry!(tokio::net::UdpSocket::bind(tunnel_udp_addr_local).await);
                    or_retry!(sock.connect(tunnel_udp_addr_remote).await);

                    let mut buf = [0u8; 65536];

                    loop {
                        tokio::select! {
                            x = rx.recv() => {
                                let Some((id, data)) = x else { break 'inner };
                                if data.len() >= 65536 - 2
                                /* size of header */
                                {
                                    println!(
                                        "[client] skipping a message for udp tunnel. can't feed header"
                                    );
                                    continue;
                                }

                                /* TODO: performance */
                                let mut buf = id.to_ne_bytes().to_vec();
                                buf.extend(data);
                                or_retry!(sock.send(buf.as_slice()).await);
                            },
                            x = sock.recv(&mut buf) => {
                                let n = or_retry!(x);
                                let data = &buf[..n];
                                let id = u16::from_ne_bytes([data[0], data[1]]);
                                let msg = &data[2..];
                                let _ = tx.send((id, msg.to_vec()));
                            }
                        }
                    }
                }
            }
        });
    }

    let flags = Arc::new({
        let mut x = vec![];
        for _ in 0..u16::MAX {
            x.push(AtomicBool::new(false));
        }
        assert_eq!(x.len(), u16::MAX as usize);
        x
    });
    while let Some(mut chan) = from_tun_chans.pop() {
        let flags = flags.clone();
        let incoming_sock = incoming_sock.clone();
        tokio::spawn(async move {
            while let Some((id, msg)) = chan.recv().await {
                if flags[id as usize].swap(true, std::sync::atomic::Ordering::SeqCst) {
                    continue; /* already sent */
                }
                let _ = incoming_sock.send(msg.as_slice()).await;
            }
        });
    }

    // incoming recv
    let mut buf = [0u8; 65536];
    let mut next_id = 0u16;
    loop {
        let (n, sender) = incoming_sock.recv_from(&mut buf).await.unwrap();
        if !incoming_sock_connected {
            incoming_sock.connect(sender).await?;
            incoming_sock_connected = true;
        }
        let data = &buf[..n];
        for chan in to_tun_chans.iter() {
            let _ = chan.send((next_id, data.to_vec()));
        }
        next_id = next_id.wrapping_add(1);
    }
}

pub enum Config {
    Clinet {
        incoming: SocketAddr,
        tunnel_tcp: SocketAddr,
        tunnel_udp_remote: SocketAddr,
        tunnel_udp_local: SocketAddr,
    },
    Server {
        incoming_tcp: SocketAddr,
        incoming_udp: SocketAddr,
        outgoing_remote: SocketAddr,
        outgoing_local: SocketAddr,
    },
}
