use std::{
    collections::HashSet,
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
            num_tcp_chans,
            num_udp_chans,
        } => {
            start_client(
                incoming,
                tunnel_tcp,
                tunnel_udp_remote,
                tunnel_udp_local,
                num_tcp_chans,
                num_udp_chans,
            )
            .await
        }
        Config::Server {
            incoming_tcp,
            incoming_udp,
            outgoing_remote,
            outgoing_local,
        } => start_server(incoming_tcp, incoming_udp, outgoing_remote, outgoing_local).await,
    }
}

async fn start_server(
    incoming_tcp_addr: SocketAddr,
    incoming_udp_addr: SocketAddr,
    outgoing_udp_remote: SocketAddr,
    outgoing_udp_local: SocketAddr,
) -> Result<()> {
    let outgoing_sock = Arc::new(tokio::net::UdpSocket::bind(outgoing_udp_local).await?);
    outgoing_sock.connect(outgoing_udp_remote).await?;

    let (new_chan_tx, mut new_chan_rx) =
        tokio::sync::mpsc::channel::<tokio::sync::mpsc::Receiver<(u16, Vec<u8>)>>(500);

    let (from_out_tx, mut from_out_rx) = tokio::sync::broadcast::channel::<(u16, Vec<u8>)>(500);

    let tcp_listener = tokio::net::TcpListener::bind(incoming_tcp_addr).await?;
    let from_out_tx_ref = from_out_tx.clone();
    let new_chan_tx_ref = new_chan_tx.clone();
    tokio::spawn(async move {
        loop {
            let (stream, addr) = tcp_listener.accept().await.unwrap();
            println!("[server] {addr:?} connected");

            let (tx, rx) = tokio::sync::mpsc::channel::<(u16, Vec<u8>)>(100);
            let _ = new_chan_tx.send(rx).await;

            let (mut read, mut write) = stream.into_split();
            tokio::spawn(async move {
                loop {
                    let id = read.read_u16().await.unwrap();
                    let size = read.read_u64().await.unwrap() as usize;
                    let mut buf = vec![0u8; size];
                    read.read_exact(&mut buf).await.unwrap();
                    // println!("  [server] new message from {addr:?} {buf:?}");
                    let _ = tx.send((id, buf)).await;
                }
            });
            let mut from_out_rx = from_out_tx_ref.subscribe();
            tokio::spawn(async move {
                while let Ok((id, msg)) = from_out_rx.recv().await {
                    write.write_u16(id).await.unwrap();
                    write.write_u64(msg.len() as _).await.unwrap();
                    write.write_all(msg.as_slice()).await.unwrap();
                }
            });
        }
    });

    let udp_sock = tokio::net::UdpSocket::bind(incoming_udp_addr).await?;
    tokio::spawn(async move {
        let mut clients: HashSet<SocketAddr> = HashSet::new();
        let mut buf = [0u8; 65536];
        let (tx, rx) = tokio::sync::mpsc::channel::<(u16, Vec<u8>)>(100);
        let _ = new_chan_tx_ref.send(rx).await;
        loop {
            tokio::select! {
                x = from_out_rx.recv() => {
                    let (id, data) = x.unwrap();
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
                    for client in clients.iter() {
                        udp_sock.send_to(buf.as_slice(), client).await.unwrap();
                    }
                },
                x = udp_sock.recv_from(&mut buf) => {
                    let (n, sender) = x.unwrap();
                    clients.insert(sender);
                    let data = &buf[..n];
                    let id = u16::from_ne_bytes([data[0], data[1]]);
                    let msg = &data[2..];
                    let _ = tx.send((id, msg.to_vec())).await;
                }
            }
        }
    });

    {
        let outgoing_sock = outgoing_sock.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 65536];
            let mut next_id = 0u16;
            let mut incoming_sock_connected = false;
            loop {
                let (n, sender) = outgoing_sock.recv_from(&mut buf).await.unwrap();
                if !incoming_sock_connected {
                    outgoing_sock.connect(sender).await.unwrap();
                    incoming_sock_connected = true;
                }
                let data = &buf[..n];
                let _ = from_out_tx.send((next_id, data.to_vec()));
                next_id = next_id.wrapping_add(1);
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
    while let Some(mut chan) = new_chan_rx.recv().await {
        let flags = flags.clone();
        let outgoing_sock = outgoing_sock.clone();
        tokio::spawn(async move {
            while let Some((id, msg)) = chan.recv().await {
                if flags[id as usize].swap(true, std::sync::atomic::Ordering::SeqCst) {
                    continue; /* already sent */
                }
                let _ = outgoing_sock.send(msg.as_slice()).await;
            }
        });
    }

    Ok(())
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
    let incoming_sock = Arc::new(tokio::net::UdpSocket::bind(incoming_addr).await?);

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
                                // println!("  [client] sending to tcp tun");
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
                                let _ = tx.send((id, buf)).await;
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
                                let _ = tx.send((id, msg.to_vec())).await;
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
        // println!("  [client] new message from {sender:?} {data:?}");
        for chan in to_tun_chans.iter() {
            let _ = chan.send((next_id, data.to_vec())).await;
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

        num_udp_chans: usize,
        num_tcp_chans: usize,
    },
    Server {
        incoming_tcp: SocketAddr,
        incoming_udp: SocketAddr,
        outgoing_remote: SocketAddr,
        outgoing_local: SocketAddr,
    },
}
