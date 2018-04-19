extern crate getopts;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate varlink;

use io_systemd_network::*;
use std::env;
use std::io;
use std::process::exit;
use std::sync::{Arc, RwLock};
use varlink::VarlinkService;
use varlink::OrgVarlinkServiceInterface;

mod io_systemd_network;

// Main

fn print_usage(program: &str, opts: getopts::Options) {
    let brief = format!("Usage: {} [--varlink=<address>] [--client]", program);
    print!("{}", opts.usage(&brief));
}

fn main() {
    let args: Vec<_> = env::args().collect();
    let program = args[0].clone();

    let mut opts = getopts::Options::new();
    opts.optopt("", "varlink", "varlink address URL", "<address>");
    opts.optflag("", "client", "run in client mode");
    opts.optflag("h", "help", "print this help menu");

    let matches = match opts.parse(&args[1..]) {
        Ok(m) => m,
        Err(f) => {
            eprintln!("{}", f.to_string());
            print_usage(&program, opts);
            return;
        }
    };

    if matches.opt_present("h") {
        print_usage(&program, opts);
        return;
    }

    let client_mode = matches.opt_present("client");

    let address = match matches.opt_str("varlink") {
        None => {
            if !client_mode {
                eprintln!("Need varlink address in server mode.");
                print_usage(&program, opts);
                return;
            }
            format!("exec:{}", program)
        }
        Some(a) => a,
    };

    let ret = match client_mode {
        true => run_client(address),
        false => run_server(address, 0),
    };

    exit(match ret {
        Ok(_) => 0,
        Err(err) => {
            eprintln!("error: {}", err);
            1
        }
    });
}

// Client

fn run_client(address: String) -> io::Result<()> {
    let conn = varlink::Connection::new(&address)?;

    let mut call = varlink::OrgVarlinkServiceClient::new(conn.clone());
    let info = call.get_info()?.recv()?;
    assert_eq!(&info.vendor, "org.varlink");
    assert_eq!(&info.product, "test service");
    assert_eq!(&info.version, "0.1");
    assert_eq!(&info.url, "http://varlink.org");
    assert_eq!(
        info.interfaces.get(1).unwrap().as_ref(),
        "io.systemd.network"
    );

    let description = call.get_interface_description("io.systemd.network".into())?
        .recv()?;

    assert!(description.description.is_some());

    let mut call = VarlinkClient::new(conn);

    match call.list()?.recv() {
        Ok(ListReply_ { netdevs: vec }) => {
            assert_eq!(vec.len(), 2);
            assert_eq!(vec[0].ifindex, 1);
            assert_eq!(vec[0].ifname, String::from("lo"));
            assert_eq!(vec[1].ifindex, 2);
            assert_eq!(vec[1].ifname, String::from("eth0"));
        }
        res => panic!("Unknown result {:?}", res),
    }

    match call.info(1)?.recv() {
        Ok(InfoReply_ {
            info:
                NetdevInfo {
                    ifindex: 1,
                    ifname: ref p,
                },
        }) if p == "lo" => {}
        res => panic!("Unknown result {:?}", res),
    }

    match call.info(2)?.recv() {
        Ok(InfoReply_ {
            info:
                NetdevInfo {
                    ifindex: 2,
                    ifname: ref p,
                },
        }) if p == "eth" => {}
        res => panic!("Unknown result {:?}", res),
    }

    match call.info(3)?.recv() {
        Err(Error_::VarlinkError_(varlink::Error::InvalidParameter(
            varlink::ErrorInvalidParameter {
                parameter: Some(ref p),
            },
        ))) if p == "ifindex" => {}
        res => panic!("Unknown result {:?}", res),
    }

    match call.info(4)?.recv() {
        Err(Error_::UnknownNetworkIfIndex(Some(UnknownNetworkIfIndexArgs_ { ifindex: 4 }))) => {}
        res => panic!("Unknown result {:?}", res),
    }

    Ok(())
}

struct MyIoSystemdNetwork {
    pub state: Arc<RwLock<i64>>,
}

impl io_systemd_network::VarlinkInterface for MyIoSystemdNetwork {
    fn info(&self, call: &mut _CallInfo, ifindex: i64) -> io::Result<()> {
        // State example
        {
            let mut number = self.state.write().unwrap();

            *number += 1;

            eprintln!("{}", *number);
        }

        match ifindex {
            1 => {
                return call.reply(NetdevInfo {
                    ifindex: 1,
                    ifname: "lo".into(),
                });
            }
            2 => {
                return call.reply(NetdevInfo {
                    ifindex: 2,
                    ifname: "eth".into(),
                });
            }
            3 => {
                return call.reply_invalid_parameter("ifindex".into());
            }
            _ => {
                return call.reply_unknown_network_if_index(ifindex);
            }
        }
    }

    fn list(&self, call: &mut _CallList) -> io::Result<()> {
        // State example
        {
            let mut number = self.state.write().unwrap();

            *number -= 1;

            eprintln!("{}", *number);
        }
        return call.reply(vec![
            Netdev {
                ifindex: 1,
                ifname: "lo".into(),
            },
            Netdev {
                ifindex: 2,
                ifname: "eth0".into(),
            },
        ]);
    }
}

fn run_server(address: String, timeout: u64) -> io::Result<()> {
    let state = Arc::new(RwLock::new(0));
    let myiosystemdnetwork = MyIoSystemdNetwork { state };
    let myinterface = io_systemd_network::new(Box::new(myiosystemdnetwork));
    let service = VarlinkService::new(
        "org.varlink",
        "test service",
        "0.1",
        "http://varlink.org",
        vec![Box::new(myinterface)],
    );

    varlink::listen(service, &address, 10, timeout)
}

#[cfg(test)]
mod test {
    use std::io;
    use std::{thread, time};

    fn run_self_test(address: String) -> io::Result<()> {
        let client_address = address.clone();

        let child = thread::spawn(move || {
            if let Err(e) = ::run_server(address, 4) {
                panic!("error: {}", e);
            }
        });

        // give server time to start
        thread::sleep(time::Duration::from_secs(1));

        let ret = ::run_client(client_address);
        if let Err(e) = ret {
            panic!("error: {}", e);
        }
        if let Err(e) = child.join() {
            Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!("{:#?}", e),
            ))
        } else {
            Ok(())
        }
    }

    #[test]
    fn test_unix() {
        assert!(run_self_test("unix:/tmp/io.systemd.network".into()).is_ok());
    }
}