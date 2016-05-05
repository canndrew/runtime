extern crate runtime;
extern crate mio;
#[macro_use]
extern crate unwrap;

use std::net::{SocketAddr, IpAddr, Ipv4Addr};
use std::os::unix::io::IntoRawFd;

use runtime::{Runtime, ResumableFunction};

fn main() {
    let mut poll = unwrap!(mio::Poll::new());
    let token = mio::Token(0);

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 45666);
    let listener = unwrap!(mio::tcp::TcpListener::bind(&addr));

    unwrap!(poll.register(&listener, token, mio::EventSet::readable(), mio::PollOpt::edge()));
    unwrap!(poll.poll(None));

    let (stream, peer_addr) = unwrap!(unwrap!(listener.accept()));
    let stream = stream.into_raw_fd();
    println!("Got a connection from {}", peer_addr);

    let result = ResumableFunction::new(move |mut runtime| {
        let mut buff = [0u8; 1024];
        loop {
            let n = unwrap!(runtime.read(stream, &mut buff[..]));
            if n == 0 {
                break;
            }
            unwrap!(runtime.write(stream, &buff[..n]));
        }
    });
    let mut resumable = match result {
        Ok(()) => panic!("It didn't try to block!"),
        Err(resumable) => resumable,
    };

    unwrap!(resumable.register(&mut poll, token));
    unwrap!(poll.poll(None));

    loop {
        unwrap!(resumable.reregister(&mut poll, token));
        unwrap!(poll.poll(None));
        if let Some(()) = resumable.resume() {
            break;
        }
    }

    println!("Peer hung up");
}

